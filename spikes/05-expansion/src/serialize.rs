//! Normalization, the adapter, and the round-trip check. Copied from Spike 1b
//! (`spikes/01-serializer/src/bin/events.rs`), with its flags settled the way the decisions
//! went, and with footnote definitions lifted out per file.

use std::collections::HashMap;

use markdown::mdast::{AlignKind, Image, Link, Node, TableCell};
use pulldown_cmark::{
    Alignment, CodeBlockKind, CowStr, Event, HeadingLevel, LinkType, Options as Flags, Parser, Tag,
    TagEnd,
};
use pulldown_cmark_to_cmark::{cmark_with_options, Options as CmarkOptions};

pub fn flags() -> Flags {
    Flags::ENABLE_TABLES
        | Flags::ENABLE_FOOTNOTES
        | Flags::ENABLE_STRIKETHROUGH
        | Flags::ENABLE_TASKLISTS
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

fn strip(node: &mut Node) {
    node.position_set(None);
    if let Some(children) = node.children_mut() {
        children.iter_mut().for_each(strip);
    }
}

fn collect_definitions(node: &Node, found: &mut HashMap<String, (String, Option<String>)>) {
    if let Node::Definition(d) = node {
        // CommonMark: the first definition of a label wins.
        found
            .entry(d.identifier.clone())
            .or_insert_with(|| (d.url.clone(), d.title.clone()));
    }
    if let Some(children) = node.children() {
        children.iter().for_each(|c| collect_definitions(c, found));
    }
}

fn last_line(node: &Node) -> Option<usize> {
    node.children()
        .and_then(|children| children.last())
        .and_then(last_line)
        .or_else(|| node.position().map(|p| p.end.line))
}

fn blank_between(a: &Node, b: &Node) -> bool {
    match (last_line(a), b.position().map(|p| p.start.line)) {
        (Some(end), Some(start)) => start > end + 1,
        _ => false,
    }
}

/// markdown reports a list as spread when two blank lines follow it, which isn't what
/// CommonMark means by loose. So work it out from the source lines, while they exist:
/// blank lines between items, or between the blocks directly inside an item.
fn recompute_spread(node: &mut Node) {
    if let Some(children) = node.children_mut() {
        children.iter_mut().for_each(recompute_spread);
    }
    if let Node::List(list) = node {
        if list.position.is_none() {
            return;
        }
        let loose = list.children.windows(2).any(|pair| blank_between(&pair[0], &pair[1]))
            || list.children.iter().any(|item| {
                item.children()
                    .is_some_and(|c| c.windows(2).any(|pair| blank_between(&pair[0], &pair[1])))
            });
        list.spread = loose;
        for item in &mut list.children {
            if let Node::ListItem(item) = item {
                item.spread = loose;
            }
        }
    }
}

/// Resolves references to inline form, makes footnote identifiers unique per use, and lifts
/// footnote definitions out of the tree, so they don't count as blocks when the file is
/// placed in inline position or when children are placed twice.
pub fn localize_references(node: &mut Node, use_id: Option<&str>) -> Vec<Node> {
    recompute_spread(node);
    let mut definitions = HashMap::new();
    collect_definitions(node, &mut definitions);
    apply_references(node, &definitions, use_id);
    let mut notes = Vec::new();
    take_notes(node, &mut notes);
    notes
}

fn take_notes(node: &mut Node, notes: &mut Vec<Node>) {
    let Some(children) = node.children_mut() else {
        return;
    };
    let mut kept = Vec::with_capacity(children.len());
    for mut child in std::mem::take(children) {
        take_notes(&mut child, notes);
        if matches!(child, Node::FootnoteDefinition(_)) {
            notes.push(child);
        } else {
            kept.push(child);
        }
    }
    *children = kept;
}

fn apply_references(
    node: &mut Node,
    definitions: &HashMap<String, (String, Option<String>)>,
    use_id: Option<&str>,
) {
    let scoped = |value: &str| use_id.map_or(value.to_string(), |id| format!("{id}-{value}"));
    match node {
        Node::FootnoteReference(r) => {
            r.identifier = scoped(&r.identifier);
            r.label = r.label.as_deref().map(scoped);
        }
        Node::FootnoteDefinition(d) => {
            d.identifier = scoped(&d.identifier);
            d.label = d.label.as_deref().map(scoped);
        }
        _ => {}
    }
    let Some(children) = node.children_mut() else {
        return;
    };
    children.retain(|c| !matches!(c, Node::Definition(_)));
    for child in children.iter_mut() {
        let replacement = match child {
            Node::LinkReference(r) => definitions.get(&r.identifier).map(|(url, title)| {
                Node::Link(Link {
                    children: std::mem::take(&mut r.children),
                    position: r.position.clone(),
                    url: url.clone(),
                    title: title.clone(),
                })
            }),
            Node::ImageReference(r) => definitions.get(&r.identifier).map(|(url, title)| {
                Node::Image(Image {
                    position: r.position.clone(),
                    alt: r.alt.clone(),
                    url: url.clone(),
                    title: title.clone(),
                })
            }),
            _ => None,
        };
        if let Some(replacement) = replacement {
            *child = replacement;
        }
        apply_references(child, definitions, use_id);
    }
}

/// Merges adjacent text and drops empty nodes.
fn merge_text(node: &mut Node) {
    let block_edges = matches!(node, Node::Paragraph(_) | Node::Heading(_) | Node::TableCell(_));
    let Some(children) = node.children_mut() else {
        return;
    };
    children.iter_mut().for_each(merge_text);
    let mut merged: Vec<Node> = Vec::with_capacity(children.len());
    for child in std::mem::take(children) {
        match child {
            Node::Text(t) if t.value.is_empty() => {}
            Node::Emphasis(e) if e.children.is_empty() => {}
            Node::Strong(s) if s.children.is_empty() => {}
            Node::Delete(d) if d.children.is_empty() => {}
            Node::Paragraph(p) if p.children.is_empty() => {}
            Node::Text(t) => {
                if let Some(Node::Text(previous)) = merged.last_mut() {
                    previous.value.push_str(&t.value);
                } else {
                    merged.push(Node::Text(t));
                }
            }
            other => merged.push(other),
        }
    }
    *children = merged;
    tidy_breaks(children);
    // A parser strips spaces at a block's edges, for example after a tag that expanded to nothing.
    if block_edges {
        if let Some(Node::Text(first)) = children.first_mut() {
            first.value = first.value.trim_start_matches([' ', '\t']).to_string();
        }
        if let Some(Node::Text(last)) = children.last_mut() {
            last.value.truncate(last.value.trim_end_matches([' ', '\t']).len());
        }
        children.retain(|c| !matches!(c, Node::Text(t) if t.value.is_empty()));
    }
}

/// A hard break has no meaning at the edge of its container, and the spaces around one
/// aren't content: a parser strips them.
fn tidy_breaks(children: &mut Vec<Node>) {
    for index in 0..children.len() {
        if !matches!(children[index], Node::Break(_)) {
            continue;
        }
        if let Some(Node::Text(before)) = index.checked_sub(1).and_then(|i| children.get_mut(i)) {
            before.value.truncate(before.value.trim_end_matches([' ', '\t']).len());
        }
        if let Some(Node::Text(after)) = children.get_mut(index + 1) {
            after.value = after.value.trim_start_matches([' ', '\t']).to_string();
        }
    }
    children.retain(|c| !matches!(c, Node::Text(t) if t.value.is_empty()));
    while matches!(children.first(), Some(Node::Break(_))) {
        children.remove(0);
    }
    while matches!(children.last(), Some(Node::Break(_))) {
        children.pop();
    }
}

/// CommonMark decides tight or loose per list, so per-item `spread` has no meaning of its own.
/// Two paragraphs side by side can only be spelled with a blank line, so a list that received
/// them through a Content Slot is loose whatever its template said. The same goes for anything
/// after a table, quote, or list, which would otherwise absorb the next line.
fn needs_blank_line(pair: &[Node]) -> bool {
    let absorbs = |n: &Node| matches!(n, Node::Table(_) | Node::Blockquote(_) | Node::List(_));
    let takes_lines = |n: &Node| matches!(n, Node::Paragraph(_) | Node::Table(_) | Node::Blockquote(_) | Node::List(_));
    matches!(pair, [Node::Paragraph(_), Node::Paragraph(_)]) || (absorbs(&pair[0]) && takes_lines(&pair[1]))
}

fn unify_spread(node: &mut Node) {
    if let Node::List(list) = node {
        let loose = list.spread
            || list.children.iter().any(|c| {
                matches!(c, Node::ListItem(i) if i.spread
                    || i.children.windows(2).any(needs_blank_line))
            });
        list.spread = loose;
        for child in &mut list.children {
            if let Node::ListItem(item) = child {
                item.spread = loose;
            }
        }
    }
    if let Some(children) = node.children_mut() {
        children.iter_mut().for_each(unify_spread);
    }
}

/// GFM pads a short row with empty cells, so a short row and a padded one mean the same.
fn pad_table_rows(node: &mut Node) {
    if let Node::Table(table) = node {
        let width = match table.children.first() {
            Some(Node::TableRow(head)) => head.children.len(),
            _ => 0,
        };
        for row in &mut table.children {
            if let Node::TableRow(row) = row {
                while row.children.len() < width {
                    row.children.push(Node::TableCell(TableCell {
                        children: vec![],
                        position: None,
                    }));
                }
            }
        }
    }
    if let Some(children) = node.children_mut() {
        children.iter_mut().for_each(pad_table_rows);
    }
}

pub fn normalize(node: &mut Node) {
    strip(node);
    merge_text(node);
    pad_table_rows(node);
    unify_spread(node);
}

// ---------------------------------------------------------------------------
// The adapter: normalized tree to pulldown-cmark events
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Context {
    Flow,
    Inline,
    TightItem,
}

fn heading_level(depth: u8) -> HeadingLevel {
    match depth {
        1 => HeadingLevel::H1,
        2 => HeadingLevel::H2,
        3 => HeadingLevel::H3,
        4 => HeadingLevel::H4,
        5 => HeadingLevel::H5,
        _ => HeadingLevel::H6,
    }
}

fn text_events(value: &str, out: &mut Vec<Event<'static>>) {
    // One line break between ordinary lines is a soft break. Anything else stays literal,
    // and the serializer encodes it, for example as &#10;.
    let plain = !value.contains("\n\n")
        && !value.starts_with('\n')
        && !value.ends_with('\n')
        && value
            .split('\n')
            .all(|line| line == line.trim_matches([' ', '\t', '\r']));
    if plain && value.contains('\n') {
        for (index, line) in value.split('\n').enumerate() {
            if index > 0 {
                out.push(Event::SoftBreak);
            }
            out.push(Event::Text(CowStr::from(line.to_string())));
        }
    } else {
        out.push(Event::Text(CowStr::from(value.to_string())));
    }
}

/// CommonMark's rule for a code span, which markdown leaves undone when a line break is
/// at the edge: line breaks become spaces, then one space comes off each side.
fn code_span_value(raw: &str) -> String {
    let edge_break = raw.starts_with(['\n', '\r']) || raw.ends_with(['\n', '\r']);
    let value = raw.replace("\r\n", " ").replace(['\n', '\r'], " ");
    let strip = edge_break
        && value.starts_with(' ')
        && value.ends_with(' ')
        && value.chars().any(|c| c != ' ');
    if strip {
        value[1..value.len() - 1].to_string()
    } else {
        value
    }
}

fn visit_all(
    nodes: &[Node],
    context: Context,
    out: &mut Vec<Event<'static>>,
) -> Result<(), String> {
    nodes.iter().try_for_each(|n| visit(n, context, out))
}

fn wrap(
    start: Tag<'static>,
    end: TagEnd,
    children: &[Node],
    context: Context,
    out: &mut Vec<Event<'static>>,
) -> Result<(), String> {
    out.push(Event::Start(start));
    visit_all(children, context, out)?;
    out.push(Event::End(end));
    Ok(())
}

fn visit(node: &Node, context: Context, out: &mut Vec<Event<'static>>) -> Result<(), String> {
    use Context::{Flow, Inline, TightItem};
    match node {
        Node::Root(n) => visit_all(&n.children, Flow, out),
        Node::Paragraph(n) if context == TightItem => visit_all(&n.children, Inline, out),
        Node::Paragraph(n) => wrap(Tag::Paragraph, TagEnd::Paragraph, &n.children, Inline, out),
        Node::Heading(n) => {
            let level = heading_level(n.depth);
            let start = Tag::Heading {
                level,
                id: None,
                classes: vec![],
                attrs: vec![],
            };
            wrap(start, TagEnd::Heading(level), &n.children, Inline, out)
        }
        Node::ThematicBreak(_) => {
            out.push(Event::Rule);
            Ok(())
        }
        Node::Blockquote(n) => wrap(
            Tag::BlockQuote(None),
            TagEnd::BlockQuote(None),
            &n.children,
            Flow,
            out,
        ),
        Node::List(n) => {
            let tight = !n.spread
                && n.children
                    .iter()
                    .all(|c| matches!(c, Node::ListItem(i) if !i.spread));
            let start = n.ordered.then(|| u64::from(n.start.unwrap_or(1)));
            out.push(Event::Start(Tag::List(start)));
            for child in &n.children {
                let Node::ListItem(item) = child else {
                    return Err("list child is not an item".into());
                };
                out.push(Event::Start(Tag::Item));
                let inner = if tight { TightItem } else { Flow };
                match (item.checked, item.children.first()) {
                    // In a loose item, the task marker goes inside the first paragraph.
                    (Some(checked), Some(Node::Paragraph(first))) if !tight => {
                        out.push(Event::Start(Tag::Paragraph));
                        out.push(Event::TaskListMarker(checked));
                        visit_all(&first.children, Inline, out)?;
                        out.push(Event::End(TagEnd::Paragraph));
                        visit_all(&item.children[1..], inner, out)?;
                    }
                    (Some(checked), _) => {
                        out.push(Event::TaskListMarker(checked));
                        visit_all(&item.children, inner, out)?;
                    }
                    (None, _) => visit_all(&item.children, inner, out)?,
                }
                out.push(Event::End(TagEnd::Item));
            }
            out.push(Event::End(TagEnd::List(n.ordered)));
            Ok(())
        }
        Node::ListItem(_) => Err("list item outside a list".into()),
        Node::Code(n) => {
            let info = match (&n.lang, &n.meta) {
                (Some(lang), Some(meta)) => format!("{lang} {meta}"),
                (Some(lang), None) => lang.clone(),
                _ => String::new(),
            };
            out.push(Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(
                CowStr::from(info),
            ))));
            if !n.value.is_empty() {
                out.push(Event::Text(CowStr::from(format!("{}\n", n.value))));
            }
            out.push(Event::End(TagEnd::CodeBlock));
            Ok(())
        }
        Node::Text(n) => {
            text_events(&n.value, out);
            Ok(())
        }
        Node::Emphasis(n) => wrap(Tag::Emphasis, TagEnd::Emphasis, &n.children, Inline, out),
        Node::Strong(n) => wrap(Tag::Strong, TagEnd::Strong, &n.children, Inline, out),
        Node::Delete(n) => wrap(
            Tag::Strikethrough,
            TagEnd::Strikethrough,
            &n.children,
            Inline,
            out,
        ),
        Node::InlineCode(n) => {
            out.push(Event::Code(CowStr::from(code_span_value(&n.value))));
            Ok(())
        }
        Node::Break(_) => {
            out.push(Event::HardBreak);
            Ok(())
        }
        Node::Link(n) => {
            let start = Tag::Link {
                link_type: LinkType::Inline,
                dest_url: CowStr::from(n.url.clone()),
                title: CowStr::from(n.title.clone().unwrap_or_default()),
                id: CowStr::from(""),
            };
            wrap(start, TagEnd::Link, &n.children, Inline, out)
        }
        Node::Image(n) => {
            out.push(Event::Start(Tag::Image {
                link_type: LinkType::Inline,
                dest_url: CowStr::from(n.url.clone()),
                title: CowStr::from(n.title.clone().unwrap_or_default()),
                id: CowStr::from(""),
            }));
            if !n.alt.is_empty() {
                out.push(Event::Text(CowStr::from(n.alt.clone())));
            }
            out.push(Event::End(TagEnd::Image));
            Ok(())
        }
        Node::Table(n) => {
            let align = n
                .align
                .iter()
                .map(|a| match a {
                    AlignKind::Left => Alignment::Left,
                    AlignKind::Right => Alignment::Right,
                    AlignKind::Center => Alignment::Center,
                    AlignKind::None => Alignment::None,
                })
                .collect();
            out.push(Event::Start(Tag::Table(align)));
            for (index, row) in n.children.iter().enumerate() {
                let Node::TableRow(row) = row else {
                    return Err("table child is not a row".into());
                };
                let (start, end) = if index == 0 {
                    (Tag::TableHead, TagEnd::TableHead)
                } else {
                    (Tag::TableRow, TagEnd::TableRow)
                };
                out.push(Event::Start(start));
                for cell in &row.children {
                    let Node::TableCell(cell) = cell else {
                        return Err("row child is not a cell".into());
                    };
                    let from = out.len();
                    wrap(Tag::TableCell, TagEnd::TableCell, &cell.children, Inline, out)?;
                    // A real line break would end the row, so the serializer must encode it.
                    for event in &mut out[from..] {
                        if matches!(event, Event::SoftBreak) {
                            *event = Event::Text(CowStr::from("\n"));
                        }
                    }
                }
                out.push(Event::End(end));
            }
            out.push(Event::End(TagEnd::Table));
            Ok(())
        }
        Node::TableRow(_) | Node::TableCell(_) => Err("table part outside a table".into()),
        Node::FootnoteDefinition(n) => wrap(
            Tag::FootnoteDefinition(CowStr::from(
                n.label.clone().unwrap_or_else(|| n.identifier.clone()),
            )),
            TagEnd::FootnoteDefinition,
            &n.children,
            Flow,
            out,
        ),
        Node::FootnoteReference(n) => {
            out.push(Event::FootnoteReference(CowStr::from(
                n.label.clone().unwrap_or_else(|| n.identifier.clone()),
            )));
            Ok(())
        }
        other => Err(format!("not normalized: {other:?}")),
    }
}

pub fn to_events(tree: &Node) -> Result<Vec<Event<'static>>, String> {
    let mut events = Vec::new();
    visit(tree, Context::Flow, &mut events)?;
    Ok(events)
}

pub fn serialize(events: &[Event<'static>]) -> Result<String, String> {
    let options = CmarkOptions {
        code_block_token_count: 3,
        list_token: '-',
        increment_ordered_list_bullets: true,
        ..CmarkOptions::default()
    };
    let mut out = String::new();
    cmark_with_options(events.iter(), &mut out, flags(), options).map_err(|e| format!("{e:?}"))?;
    Ok(out)
}

/// Link types and split text don't change meaning, so compare without them.
pub fn canonical(events: impl IntoIterator<Item = Event<'static>>) -> Vec<Event<'static>> {
    let mut out: Vec<Event<'static>> = Vec::new();
    for event in events {
        let event = match event {
            Event::Start(Tag::Link {
                dest_url, title, ..
            }) => Event::Start(Tag::Link {
                link_type: LinkType::Inline,
                dest_url,
                title,
                id: CowStr::from(""),
            }),
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => Event::Start(Tag::Image {
                link_type: LinkType::Inline,
                dest_url,
                title,
                id: CowStr::from(""),
            }),
            // markdown keeps a line break in a code span's value, and pulldown-cmark
            // turns it into a space. CommonMark renders both as a space.
            Event::Code(code) => {
                Event::Code(CowStr::from(code.replace("\r\n", " ").replace('\n', " ")))
            }
            other => other,
        };
        match (&mut out.last_mut(), event) {
            (Some(Event::Text(previous)), Event::Text(next)) => {
                *previous = CowStr::from(format!("{previous}{next}"));
            }
            (_, event) => out.push(event),
        }
    }
    out
}

pub fn parse_markdown(text: &str) -> Vec<Event<'static>> {
    canonical(Parser::new_ext(text, flags()).map(|event| event.into_static()))
}

/// The first place two event lists differ, for a message.
pub fn first_difference(a: &[Event<'static>], b: &[Event<'static>]) -> String {
    let index = a.iter().zip(b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
    let show = |events: &[Event<'static>]| events.get(index).map_or("the end".to_string(), |e| format!("{e:?}"));
    format!("expected {}, the Markdown reads as {}", show(a), show(b))
}
