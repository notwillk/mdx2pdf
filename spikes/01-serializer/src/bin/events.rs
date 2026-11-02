//! Spike 1b: pulldown-cmark-to-cmark through a small adapter, with normalization.
//!
//! Usage: events <label> [--literal-autolinks] [--no-spread-rule]
//! Writes results/<label>/{summary.txt,misses.txt,outputs.txt}.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use markdown::mdast::{
    AlignKind, AttributeContent, AttributeValue, Blockquote, Delete, Emphasis, Heading, InlineCode,
    Link, List, ListItem, Node, Paragraph, Root, Strong, Table, TableCell, TableRow, Text,
};
use markdown::{to_mdast, Constructs, ParseOptions};
use pulldown_cmark::{
    Alignment, CodeBlockKind, CowStr, Event, HeadingLevel, LinkType, Options as Flags, Parser, Tag,
    TagEnd,
};
use pulldown_cmark_to_cmark::{cmark_with_options, Options as CmarkOptions};

include!("../suites.rs");

#[derive(Clone, Copy)]
enum Grammar {
    CommonMark,
    Gfm,
    Dialect,
}

#[derive(Clone, Copy)]
struct Config {
    literal_autolinks: bool,
    spread_rule: bool,
    raw_values: bool,
}

fn parse_options(grammar: Grammar, config: Config) -> ParseOptions {
    let gfm = Constructs {
        gfm_autolink_literal: config.literal_autolinks,
        ..Constructs::gfm()
    };
    match grammar {
        Grammar::CommonMark => ParseOptions::default(),
        Grammar::Gfm => ParseOptions {
            constructs: gfm,
            ..ParseOptions::gfm()
        },
        Grammar::Dialect => ParseOptions {
            constructs: Constructs {
                mdx_jsx_flow: true,
                mdx_jsx_text: true,
                mdx_expression_flow: true,
                mdx_expression_text: true,
                mdx_esm: false,
                html_flow: false,
                html_text: false,
                code_indented: false,
                autolink: false,
                ..gfm
            },
            ..ParseOptions::gfm()
        },
    }
}

fn flags(grammar: Grammar) -> Flags {
    match grammar {
        Grammar::CommonMark => Flags::empty(),
        _ => {
            Flags::ENABLE_TABLES
                | Flags::ENABLE_FOOTNOTES
                | Flags::ENABLE_STRIKETHROUGH
                | Flags::ENABLE_TASKLISTS
        }
    }
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

/// Resolves references to inline form and makes footnote identifiers unique per use.
fn localize_references(node: &mut Node, use_id: Option<&str>) {
    if STAGE6_EVENTS.with(|flag| *flag.borrow()) {
        recompute_spread(node);
    }
    let mut definitions = HashMap::new();
    collect_definitions(node, &mut definitions);
    apply_references(node, &definitions, use_id);
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
                    position: None,
                    url: url.clone(),
                    title: title.clone(),
                })
            }),
            Node::ImageReference(r) => definitions.get(&r.identifier).map(|(url, title)| {
                Node::Image(markdown::mdast::Image {
                    position: None,
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

fn clear_spread(node: &mut Node) {
    match node {
        Node::List(list) => list.spread = false,
        Node::ListItem(item) => item.spread = false,
        _ => {}
    }
    if let Some(children) = node.children_mut() {
        children.iter_mut().for_each(clear_spread);
    }
}

/// CommonMark decides tight or loose per list, so per-item `spread` has no meaning of its own.
/// Two paragraphs side by side can only be spelled with a blank line, so a list that
/// received them through a Content Slot is loose whatever its template said.
fn unify_spread(node: &mut Node) {
    if let Node::List(list) = node {
        let loose = list.spread
            || list.children.iter().any(|c| {
                matches!(c, Node::ListItem(i) if i.spread
                    || i.children.windows(2).any(|pair| matches!(pair, [Node::Paragraph(_), Node::Paragraph(_)])))
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

fn normalize(node: &mut Node, config: Config) {
    strip(node);
    merge_text(node);
    pad_table_rows(node);
    if config.spread_rule {
        unify_spread(node);
    }
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
            let value = if STAGE6_EVENTS.with(|flag| *flag.borrow()) {
                code_span_value(&n.value)
            } else {
                n.value.clone()
            };
            out.push(Event::Code(CowStr::from(value)));
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
        // Html can't reach Stage 5. Mapped only so the spec suites stay comparable to Spike 1.
        Node::Html(n) if context == Inline => {
            out.push(Event::InlineHtml(CowStr::from(n.value.clone())));
            Ok(())
        }
        Node::Html(n) => {
            out.push(Event::Start(Tag::HtmlBlock));
            for line in n.value.split('\n') {
                out.push(Event::Html(CowStr::from(format!("{line}\n"))));
            }
            out.push(Event::End(TagEnd::HtmlBlock));
            Ok(())
        }
        other => Err(format!("not normalized: {other:?}")),
    }
}

fn to_events(tree: &Node) -> Result<Vec<Event<'static>>, String> {
    let mut events = Vec::new();
    visit(tree, Context::Flow, &mut events)?;
    Ok(events)
}

fn serialize(events: &[Event<'static>], grammar: Grammar) -> Result<String, String> {
    let options = CmarkOptions {
        code_block_token_count: 3,
        list_token: '-',
        increment_ordered_list_bullets: STAGE6_EVENTS.with(|flag| *flag.borrow()),
        ..CmarkOptions::default()
    };
    let mut out = String::new();
    cmark_with_options(events.iter(), &mut out, flags(grammar), options)
        .map_err(|e| format!("{e:?}"))?;
    Ok(out)
}

/// Link types and split text don't change meaning, so compare without them.
fn canonical(events: impl IntoIterator<Item = Event<'static>>) -> Vec<Event<'static>> {
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
            Event::Code(code) => Event::Code(CowStr::from(code.replace("\r\n", " ").replace('\n', " "))),
            other => other,
        };
        match (&mut out.last_mut(), event) {
            (Some(Event::Text(previous)), Event::Text(next)) => {
                *previous = CowStr::from(format!("{previous}{next}"));
            }
            (Some(Event::Html(previous)), Event::Html(next)) => {
                *previous = CowStr::from(format!("{previous}{next}"));
            }
            (_, event) => out.push(event),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

thread_local! {
    static PANIC_MESSAGE: RefCell<String> = const { RefCell::new(String::new()) };
    static PULLDOWN_PANICS: RefCell<usize> = const { RefCell::new(0) };
    static STAGE6_EVENTS: RefCell<bool> = const { RefCell::new(false) };
}

/// The characters a Markdown parser strips from the edges of a block.
const EDGE: [char; 4] = [' ', '\t', '\n', '\r'];

fn guarded<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work))
        .map_err(|_| PANIC_MESSAGE.with(|message| message.borrow().clone()))
}

fn first_difference(a: &str, b: &str) -> String {
    let index = a
        .char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((index, _), _)| index);
    let start = a.floor_char_boundary(index.saturating_sub(70));
    let tail = |s: &str| {
        s.get(start..)
            .unwrap_or("")
            .chars()
            .take(170)
            .collect::<String>()
    };
    format!("before: ...{}\nafter:  ...{}", tail(a), tail(b))
}

#[derive(Default)]
struct Outcome {
    kind: &'static str,
    pd_equal: bool,
    serialized: Option<String>,
    detail: String,
}

fn has_html(node: &Node) -> bool {
    matches!(node, Node::Html(_)) || node.children().is_some_and(|c| c.iter().any(has_html))
}

/// Serializes a normalized tree, re-parses under both parsers, and compares.
fn check(tree: &Node, second: Grammar, config: Config) -> Outcome {
    let mut outcome = Outcome {
        kind: "equal",
        ..Outcome::default()
    };
    let events = match to_events(tree) {
        Ok(events) => events,
        Err(message) => {
            outcome.kind = "adapter_err";
            outcome.detail = message.chars().take(200).collect();
            return outcome;
        }
    };
    let serialized = match guarded(|| serialize(&events, second)) {
        Err(message) => {
            outcome.kind = "panic";
            outcome.detail = message;
            return outcome;
        }
        Ok(Err(message)) => {
            outcome.kind = "serialize_err";
            outcome.detail = message;
            return outcome;
        }
        Ok(Ok(serialized)) => serialized,
    };
    outcome.serialized = Some(serialized.clone());

    let reparsed = match guarded(|| to_mdast(&serialized, &parse_options(second, config))) {
        Err(message) => {
            outcome.kind = "panic";
            outcome.detail = format!("in second parse: {message}");
            return outcome;
        }
        Ok(Err(message)) => {
            outcome.kind = "parse_err";
            outcome.detail = format!("second parse: {message}");
            return outcome;
        }
        Ok(Ok(reparsed)) => reparsed,
    };
    let mut reparsed = reparsed;
    localize_references(&mut reparsed, None);
    normalize(&mut reparsed, config);
    if &reparsed != tree {
        let (mut a, mut b) = (tree.clone(), reparsed.clone());
        clear_spread(&mut a);
        clear_spread(&mut b);
        outcome.kind = if a == b { "spread_only" } else { "mismatch" };
        outcome.detail = first_difference(&format!("{tree:?}"), &format!("{reparsed:?}"));
    }

    let parsed = guarded(|| {
        canonical(
            Parser::new_ext(&serialized, flags(second)).map(|event| event.into_static()),
        )
    });
    if parsed.is_err() {
        PULLDOWN_PANICS.with(|count| *count.borrow_mut() += 1);
    }
    outcome.pd_equal = parsed.is_ok_and(|parsed| parsed == canonical(events));
    outcome
}

struct Record {
    suite: &'static str,
    name: String,
    input: String,
    outcome: Outcome,
    html: bool,
    note: &'static str,
}

fn from_source(
    suite: &'static str,
    name: String,
    input: &str,
    first: Grammar,
    second: Grammar,
    config: Config,
) -> Option<Record> {
    let tree = guarded(|| to_mdast(input, &parse_options(first, config)));
    let mut tree = match tree {
        Err(message) => {
            return Some(Record {
                suite,
                name,
                input: input.into(),
                outcome: Outcome {
                    kind: "panic",
                    detail: format!("in first parse: {message}"),
                    ..Outcome::default()
                },
                html: false,
                note: "",
            })
        }
        // A Dialect parse error is a normal rejected document, not a serializer result.
        Ok(Err(_)) => return None,
        Ok(Ok(tree)) => tree,
    };
    if matches!(first, Grammar::Dialect) {
        unwrap_mdx(&mut tree);
    }
    localize_references(&mut tree, None);
    normalize(&mut tree, config);
    let html = has_html(&tree);
    Some(Record {
        suite,
        name,
        input: input.into(),
        outcome: check(&tree, second, config),
        html,
        note: "",
    })
}

// ---------------------------------------------------------------------------
// A mock of Expansion, just enough to produce realistic trees
// ---------------------------------------------------------------------------

/// Stands in for Expansion on arbitrary parsed input: tags become their children and
/// expressions become their own text.
fn unwrap_mdx(node: &mut Node) {
    let Some(children) = node.children_mut() else {
        return;
    };
    let mut out = Vec::with_capacity(children.len());
    for mut child in std::mem::take(children) {
        unwrap_mdx(&mut child);
        match child {
            Node::MdxJsxFlowElement(e) => out.extend(e.children),
            Node::MdxJsxTextElement(e) => out.extend(e.children),
            Node::MdxFlowExpression(e) => out.push(paragraph(&e.value)),
            Node::MdxTextExpression(e) => out.push(text(&e.value)),
            other => out.push(other),
        }
    }
    *children = out;
}

struct Project<'a> {
    files: BTreeMap<&'a str, &'a str>,
    scope: BTreeMap<&'a str, &'a str>,
    config: Config,
}

struct Frame {
    props: BTreeMap<String, String>,
    children: Vec<Node>,
}

impl Project<'_> {
    fn evaluate(&self, expression: &str, frame: &Frame) -> Result<String, String> {
        let mut parts = expression.split('|').map(str::trim);
        let key = parts.next().unwrap_or("");
        let value = if let Some(name) = key.strip_prefix("props.") {
            frame.props.get(name).cloned()
        } else {
            let name = key.strip_prefix("meta.").unwrap_or(key);
            self.scope.get(name).map(|v| (*v).to_string())
        };
        let mut value = value.ok_or(format!("undefined value `{key}`"))?;
        for filter in parts {
            match filter {
                "upcase" => value = value.to_uppercase(),
                other => return Err(format!("unknown filter `{other}`")),
            }
        }
        // A value's edge whitespace has no Markdown spelling inside emphasis or at a
        // block's edge, so Expressions trim it.
        Ok(value.trim_matches(EDGE).to_string())
    }

    fn instantiate(
        &self,
        name: &str,
        frame: Frame,
        uses: &mut usize,
        stack: &mut Vec<String>,
    ) -> Result<Vec<Node>, String> {
        if stack.iter().any(|s| s == name) {
            return Err(format!("recursion: {stack:?} -> {name}"));
        }
        let source = self
            .files
            .get(name)
            .ok_or(format!("unknown component `{name}`"))?;
        let mut tree = to_mdast(source, &parse_options(Grammar::Dialect, self.config))
            .map_err(|m| format!("{name}: {m}"))?;
        *uses += 1;
        localize_references(&mut tree, Some(&format!("u{uses}")));
        stack.push(name.to_string());
        let Node::Root(root) = tree else {
            return Err("no root".into());
        };
        let result = self.expand(root.children, &frame, uses, stack);
        stack.pop();
        result
    }

    fn attributes(
        &self,
        attributes: &[AttributeContent],
        frame: &Frame,
    ) -> Result<BTreeMap<String, String>, String> {
        let mut props = BTreeMap::new();
        for attribute in attributes {
            let AttributeContent::Property(property) = attribute else {
                return Err("spread attribute".into());
            };
            let value = match &property.value {
                None => "true".to_string(),
                Some(AttributeValue::Literal(value)) => value.clone(),
                Some(AttributeValue::Expression(e)) => self.evaluate(&e.value, frame)?,
            };
            props.insert(property.name.clone(), value);
        }
        Ok(props)
    }

    fn expand(
        &self,
        nodes: Vec<Node>,
        frame: &Frame,
        uses: &mut usize,
        stack: &mut Vec<String>,
    ) -> Result<Vec<Node>, String> {
        let mut out = Vec::with_capacity(nodes.len());
        for node in nodes {
            match node {
                Node::MdxFlowExpression(e) => out.push(paragraph(&self.evaluate(&e.value, frame)?)),
                Node::MdxTextExpression(e) => out.push(text(&self.evaluate(&e.value, frame)?)),
                Node::MdxJsxFlowElement(e) => {
                    let name = e.name.clone().unwrap_or_default();
                    if name == "Content" {
                        out.extend(frame.children.clone());
                        continue;
                    }
                    let children = self.expand(e.children, frame, uses, stack)?;
                    let props = self.attributes(&e.attributes, frame)?;
                    out.extend(self.instantiate(&name, Frame { props, children }, uses, stack)?);
                }
                Node::MdxJsxTextElement(e) => {
                    let name = e.name.clone().unwrap_or_default();
                    let blocks = if name == "Content" {
                        frame.children.clone()
                    } else {
                        let children = self.expand(e.children, frame, uses, stack)?;
                        let props = self.attributes(&e.attributes, frame)?;
                        self.instantiate(&name, Frame { props, children }, uses, stack)?
                    };
                    // Inline position takes a single paragraph's contents, or plain inlines.
                    match <[Node; 1]>::try_from(blocks) {
                        Ok([Node::Paragraph(p)]) => out.extend(p.children),
                        Ok([other]) => out.push(other),
                        Err(blocks) if blocks.is_empty() => {}
                        Err(_) => return Err(format!("`{name}` expands to blocks in inline position")),
                    }
                }
                // Unravel: a paragraph of only tags and whitespace is block level.
                Node::Paragraph(p)
                    if p.children.iter().any(|c| matches!(c, Node::MdxJsxTextElement(_)))
                        && p.children.iter().all(|c| match c {
                            Node::MdxJsxTextElement(_) => true,
                            Node::Text(t) => t.value.trim().is_empty(),
                            _ => false,
                        }) =>
                {
                    for child in p.children {
                        let Node::MdxJsxTextElement(e) = child else {
                            continue;
                        };
                        let name = e.name.clone().unwrap_or_default();
                        if name == "Content" {
                            out.extend(frame.children.clone());
                            continue;
                        }
                        let children = self.expand(e.children, frame, uses, stack)?;
                        let props = self.attributes(&e.attributes, frame)?;
                        out.extend(self.instantiate(&name, Frame { props, children }, uses, stack)?);
                    }
                }
                mut other => {
                    if let Some(children) = other.children_mut() {
                        *children = self.expand(std::mem::take(children), frame, uses, stack)?;
                    }
                    out.push(other);
                }
            }
        }
        Ok(out)
    }

    fn run(&self) -> Result<Node, String> {
        let frame = Frame {
            props: BTreeMap::new(),
            children: vec![],
        };
        let mut tree = root(self.instantiate("index", frame, &mut 0, &mut vec![])?);
        normalize(&mut tree, self.config);
        Ok(tree)
    }
}

// ---------------------------------------------------------------------------
// Suite F: Expansion-shaped trees
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Expect {
    /// Must round-trip.
    Equal,
    /// Has no Markdown form, so the round-trip check must report it.
    Caught,
    /// Expansion itself must reject it.
    ExpandError,
}

struct Case {
    name: &'static str,
    files: &'static [(&'static str, &'static str)],
    scope: &'static [(&'static str, &'static str)],
    expect: Expect,
    contains: &'static [&'static str],
}

const CALLOUT: &str = "> **{props.kind | upcase}**\n>\n> <Content />\n";
const PRD_INDEX: &str = "# {meta.title}\n\n<Callout kind=\"Alpha\">\nReview **Alpha** before {meta.title} ships.\n</Callout>\n\n<Callout kind=\"Beta\">\nReview **Beta** before {meta.title} ships.\n</Callout>\n";
const PRD_OUTPUT: &str = "# Q3 Report\n\n> **ALPHA**\n>\n> Review **Alpha** before Q3 Report ships.\n\n> **BETA**\n>\n> Review **Beta** before Q3 Report ships.\n";

fn expansion_cases() -> Vec<Case> {
    use Expect::{Caught, Equal, ExpandError};
    let c = |name, files, scope, expect, contains| Case {
        name,
        files,
        scope,
        expect,
        contains,
    };
    vec![
        c("PRD example", &[("index", PRD_INDEX), ("Callout", CALLOUT)], &[("title", "Q3 Report")], Equal, &[]),
        c("adjacent text", &[("index", "a{x}b\n")], &[("x", "1")], Equal, &["a1b"]),
        c("value with emphasis characters", &[("index", "a{x}b\n")], &[("x", "*y*")], Equal, &[]),
        c("value with a tag", &[("index", "a {x} b\n")], &[("x", "A<B>c</B>")], Equal, &[]),
        c("value beside non-ascii", &[("index", "é{x}é\n")], &[("x", "*x*")], Equal, &[]),
        c("value that looks like a heading", &[("index", "{x}\n")], &[("x", "# h")], Equal, &[]),
        c("value that looks like a list", &[("index", "{x}\n")], &[("x", "- i")], Equal, &[]),
        c("value that looks like a numbered list", &[("index", "{x}\n")], &[("x", "1. i")], Equal, &[]),
        c("value that looks like a quote", &[("index", "{x}\n")], &[("x", "> q")], Equal, &[]),
        c("value that looks like a rule", &[("index", "{x}\n")], &[("x", "---")], Equal, &[]),
        c("value that looks like a fence", &[("index", "{x}\n")], &[("x", "```")], Equal, &[]),
        c("value that looks like a definition", &[("index", "{x}\n")], &[("x", "[a]: /b")], Equal, &[]),
        c("value that looks like a table row", &[("index", "{x}\n")], &[("x", "| a |")], Equal, &[]),
        c("value in a heading", &[("index", "# {x}\n")], &[("x", "# y #")], Equal, &[]),
        c("value in a table cell", &[("index", "| a |\n|---|\n| {x} |\n")], &[("x", "b | c")], Equal, &[]),
        c("value with a blank line", &[("index", "a {x} b\n")], &[("x", "c\n\nd")], Equal, &[]),
        c("value with a line break", &[("index", "a {x} b\n")], &[("x", "c\n# d")], Equal, &[]),
        c("value with edge spaces in emphasis", &[("index", "*{x}*\n")], &[("x", " a ")], Equal, &[]),
        c("value that is a url", &[("index", "a {x} b\n")], &[("x", "https://a.b")], Equal, &[]),
        c("empty value in emphasis", &[("index", "a *{x}* b\n")], &[("x", "")], Equal, &[]),
        c("empty value alone", &[("index", "a\n\n{x}\n\nb\n")], &[("x", "")], Equal, &[]),
        c("attribute value with a quote", &[("index", "<S t={x} />\n"), ("S", "*{props.t}*\n")], &[("x", "Say \"hi\"")], Equal, &[]),
        c("content in a quote, several blocks", &[("index", "<Q>\n\none\n\n- two\n- three\n\n```\nfour\n```\n\n</Q>\n"), ("Q", "> <Content />\n")], &[], Equal, &[]),
        c("content in a list item", &[("index", "<L>\n\none\n\ntwo\n\n</L>\n"), ("L", "- <Content />\n- after\n")], &[], Equal, &[]),
        c("content in a nested quote", &[("index", "<Q>\n\none\n\ntwo\n\n</Q>\n"), ("Q", "> > <Content />\n")], &[], Equal, &[]),
        c("content in a table cell", &[("index", "<T>one</T>\n"), ("T", "| a |\n|---|\n| <Content /> |\n")], &[], Equal, &[]),
        c("component passing children on", &[("index", "<A>\n\nhi {x}\n\n</A>\n"), ("A", "<B>\n\n<Content />\n\n</B>\n"), ("B", "> <Content />\n")], &[("x", "1")], Equal, &[]),
        c("inline component", &[("index", "a <B /> c\n"), ("B", "**b**\n")], &[], Equal, &["a **b** c"]),
        c("inline component beside words", &[("index", "a<B />c\n"), ("B", "**b**\n")], &[], Equal, &["a**b**c"]),
        c("same label in two components", &[("index", "<A />\n\n<B />\n"), ("A", "[docs]\n\n[docs]: /a\n"), ("B", "[docs]\n\n[docs]: /b\n")], &[], Equal, &["(/a)", "(/b)"]),
        c("reference label with emphasis", &[("index", "[*foo* bar]\n\n[*foo* bar]: /u\n")], &[], Equal, &["(/u)"]),
        c("footnote component used twice", &[("index", "<F />\n\n<F />\n"), ("F", "n[^1]\n\n[^1]: note\n")], &[], Equal, &["[^u2-1]", "[^u3-1]"]),
        c("code fence in a component in a list", &[("index", "- <C />\n"), ("C", "```rust\nfn x() {}\n```\n")], &[], Equal, &[]),
        c("nested emphasis", &[("index", "*_a_* and _*b*_\n")], &[], Equal, &[]),
        c("mid-word emphasis of punctuation", &[("index", "x<B />y\n"), ("B", "*(a)*\n")], &[], Equal, &[]),
        c("adjacent emphasis", &[("index", "<B /><B /> x\n"), ("B", "*a*\n")], &[], Equal, &[]),
        c("link inside a link", &[("index", "[<L />](/c)\n"), ("L", "[a](/b)\n")], &[], Caught, &[]),
        c("blocks in inline position", &[("index", "a <N /> b\n"), ("N", "one\n\ntwo\n")], &[], ExpandError, &[]),
        c("direct recursion", &[("index", "<A />\n"), ("A", "<A />\n")], &[], ExpandError, &[]),
        c("indirect recursion", &[("index", "<A />\n"), ("A", "<B />\n"), ("B", "<A />\n")], &[], ExpandError, &[]),
    ]
}

fn run_expansion_case(case: &Case, config: Config) -> Record {
    let project = Project {
        files: case.files.iter().copied().collect(),
        scope: case.scope.iter().copied().collect(),
        config,
    };
    let input = format!("{:?} scope={:?}", case.files, case.scope);
    let record = |outcome: Outcome, note| Record {
        suite: "F",
        name: case.name.to_string(),
        input: input.clone(),
        outcome,
        html: false,
        note,
    };
    let tree = match guarded(|| project.run()) {
        Err(message) => {
            return record(
                Outcome {
                    kind: "panic",
                    detail: message,
                    ..Outcome::default()
                },
                "UNEXPECTED",
            )
        }
        Ok(Err(message)) => {
            let note = if case.expect == Expect::ExpandError { "as expected" } else { "UNEXPECTED" };
            return record(
                Outcome {
                    kind: "expand_err",
                    detail: message,
                    ..Outcome::default()
                },
                note,
            );
        }
        Ok(Ok(tree)) => tree,
    };
    let outcome = check(&tree, Grammar::Gfm, config);
    let missing = outcome
        .serialized
        .as_deref()
        .is_some_and(|s| case.contains.iter().any(|c| !s.contains(c)));
    let note = match (case.expect, outcome.kind) {
        (Expect::Equal, "equal") if !missing => "as expected",
        (Expect::Caught, kind) if kind != "equal" && kind != "panic" => "as expected",
        _ => "UNEXPECTED",
    };
    record(outcome, note)
}

// ---------------------------------------------------------------------------
// Suites G and H: randomized
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit as u64) as usize
    }

    fn pick<'a>(&mut self, tokens: &[&'a str]) -> &'a str {
        tokens[self.below(tokens.len())]
    }
}

const PROSE_TOKENS: &[&str] = &[
    "a", "b", "word", "two words", "*", "**", "_", "__", "~", "~~", "`", "``", "[", "]", "(", ")",
    "![", "](", "<", ">", "</", "/>", "&", "&amp;", "&#10;", "#", "# ", "-", "- ", "+ ", "1. ",
    "1) ", "> ", "|", "\\", "\\*", "=", "===", "---", "{", "}", "{{", "}}", "{%", "%}", "$", "!",
    "\"", "'", ":", "https://a.b", "www.a.b", "a@b.c", "é", "日本語", "при", "🍩", "[x] ", "[ ] ",
    "[^1]", "[a]: b", ".", ",", "?", "\u{a0}", "\u{200b}",
];
const HOSTILE_TOKENS: &[&str] = &[" ", "  ", "\t", "\n", "\n\n", "    ", "\r\n", "\n# ", "\n- ", "\n> ", "\n```\n"];

const CONTAINERS: &[&str] = &[
    "paragraph", "heading", "tight item", "loose item", "quote", "table cell", "emphasis", "strong",
    "delete", "link text", "beside emphasis",
];

fn in_container(container: &str, value: &str) -> Node {
    let t = || text(value);
    let inline = |children: Vec<Node>| {
        root(vec![Node::Paragraph(Paragraph {
            children,
            position: None,
        })])
    };
    let item = |children: Vec<Node>, spread: bool| {
        Node::ListItem(ListItem {
            children,
            position: None,
            spread,
            checked: None,
        })
    };
    match container {
        "paragraph" => inline(vec![t()]),
        "heading" => root(vec![Node::Heading(Heading {
            children: vec![t()],
            position: None,
            depth: 2,
        })]),
        "tight item" => root(vec![Node::List(List {
            children: vec![item(vec![paragraph(value)], false)],
            position: None,
            ordered: false,
            start: None,
            spread: false,
        })]),
        "loose item" => root(vec![Node::List(List {
            children: vec![item(vec![paragraph(value), paragraph("z")], true)],
            position: None,
            ordered: false,
            start: None,
            spread: true,
        })]),
        "quote" => root(vec![Node::Blockquote(Blockquote {
            children: vec![paragraph(value)],
            position: None,
        })]),
        "table cell" => {
            let row = |v: &str| {
                Node::TableRow(TableRow {
                    children: vec![Node::TableCell(TableCell {
                        children: vec![text(v)],
                        position: None,
                    })],
                    position: None,
                })
            };
            root(vec![Node::Table(Table {
                children: vec![row("h"), row(value)],
                position: None,
                align: vec![AlignKind::None],
            })])
        }
        "emphasis" => inline(vec![Node::Emphasis(Emphasis {
            children: vec![t()],
            position: None,
        })]),
        "strong" => inline(vec![Node::Strong(Strong {
            children: vec![t()],
            position: None,
        })]),
        "delete" => inline(vec![Node::Delete(Delete {
            children: vec![t()],
            position: None,
        })]),
        "link text" => inline(vec![Node::Link(Link {
            children: vec![t()],
            position: None,
            url: "/u".into(),
            title: None,
        })]),
        _ => inline(vec![
            t(),
            Node::Emphasis(Emphasis {
                children: vec![text("e")],
                position: None,
            }),
            text(" z"),
        ]),
    }
}

const DOC_TOKENS: &[&str] = &[
    "a", "b c", "word ", " ", "*", "**", "_", "__", "~~", "`", "``", "[", "]", "](/u)", "](/u \"t\")",
    "![", "[r]", "[r][]", "[^1]", "<", ">", "&amp;", "&#35;", "\\", "\\*", "é", "日本語", "при", "🍩",
    "https://a.b ", "\n", "\n\n", "\n\n> ", "\n> ", "\n- ", "\n  - ", "\n* ", "\n1. ", "\n2) ",
    "\n- [x] ", "\n- [ ] ", "\n# ", "\n## ", "\n---\n", "\n===\n", "\n```\n", "\n```rust\n", "\n~~~\n",
    "\n    ", "\n| a | b |\n|---|:-:|\n| ", "  \n", "\\\n", "\n[r]: /v \"t\"\n", "\n[^1]: note\n", "|",
];

fn random_source(rng: &mut Rng, tokens: &[&str], limit: usize) -> String {
    let count = 1 + rng.below(limit);
    (0..count).map(|_| rng.pick(tokens)).collect()
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        PANIC_MESSAGE.with(|message| *message.borrow_mut() = info.to_string());
    }));

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--show") {
        // Prints what each source becomes through the production path.
        STAGE6_EVENTS.with(|flag| *flag.borrow_mut() = true);
        let config = Config {
            literal_autolinks: false,
            spread_rule: true,
            raw_values: false,
        };
        for source in &args[1..] {
            match from_source("show", String::new(), source, Grammar::Dialect, Grammar::Gfm, config) {
                None => println!("{source:?}: rejected by the Dialect"),
                Some(record) => println!(
                    "{source:?}\n   pulldown equal: {}   markdown: {}\n   output: {:?}",
                    record.outcome.pd_equal, record.outcome.kind, record.outcome.serialized
                ),
            }
        }
        return;
    }
    if args.iter().any(|arg| arg == "--stage6-events") {
        STAGE6_EVENTS.with(|flag| *flag.borrow_mut() = true);
    }
    let label = args.first().expect("usage: events <label> [flags]");
    let mut config = Config {
        literal_autolinks: false,
        spread_rule: true,
        raw_values: false,
    };
    for flag in &args[1..] {
        match flag.as_str() {
            "--literal-autolinks" => config.literal_autolinks = true,
            "--no-spread-rule" => config.spread_rule = false,
            "--raw-values" => config.raw_values = true,
            "--stage6-events" => {}
            other => panic!("unknown flag {other}"),
        }
    }

    let options = CmarkOptions {
        code_block_token_count: 3,
        list_token: '-',
        ..CmarkOptions::default()
    };
    options
        .validate(flags(Grammar::Gfm))
        .expect("the options and parser flags are supported");

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifest.join("spec.json")).unwrap()).unwrap();
    let spec = spec.as_array().unwrap();
    assert_eq!(spec.len(), 652, "CommonMark 0.31.2 has 652 examples");

    let mut records = Vec::new();
    for example in spec {
        let input = example["markdown"].as_str().unwrap();
        let name = format!("example {} ({})", example["example"], example["section"].as_str().unwrap());
        records.extend(from_source("A", name.clone(), input, Grammar::CommonMark, Grammar::CommonMark, config));
        records.extend(from_source("B", name.clone(), input, Grammar::Gfm, Grammar::Gfm, config));
        records.extend(from_source("P", name, input, Grammar::Dialect, Grammar::Gfm, config));
    }
    for (name, input) in curated() {
        records.extend(from_source("C", name.into(), &input, Grammar::Gfm, Grammar::Gfm, config));
        records.extend(from_source("P", format!("curated: {name}"), &input, Grammar::Dialect, Grammar::Gfm, config));
    }
    for (name, input) in dialect_cases() {
        records.extend(from_source("D", name.into(), input, Grammar::Dialect, Grammar::Gfm, config));
    }
    for (name, mut tree) in hand_built() {
        normalize(&mut tree, config);
        records.push(Record {
            suite: "E",
            name,
            input: format!("{tree:?}"),
            outcome: check(&tree, Grammar::Gfm, config),
            html: false,
            note: "",
        });
    }
    for case in expansion_cases() {
        records.push(run_expansion_case(&case, config));
    }

    // The PRD's exact output, end to end through the mock.
    let prd = {
        let project = Project {
            files: [("index", PRD_INDEX), ("Callout", CALLOUT)].into_iter().collect(),
            scope: [("title", "Q3 Report")].into_iter().collect(),
            config,
        };
        let tree = project.run().expect("the PRD example expands");
        serialize(&to_events(&tree).unwrap(), Grammar::Gfm).unwrap()
    };
    let prd_exact = prd.trim_end() == PRD_OUTPUT.trim_end();

    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    const VALUES: usize = 20_000;
    const DOCS: usize = 50_000;
    let mut prose_tally: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut hostile_tally: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    for _ in 0..VALUES {
        let count = 1 + rng.below(8);
        let value: String = (0..count).map(|_| rng.pick(PROSE_TOKENS)).collect();
        let value = value.trim().to_string();
        let mut hostile = String::new();
        for _ in 0..count {
            let tokens = if rng.below(3) == 0 { HOSTILE_TOKENS } else { PROSE_TOKENS };
            hostile.push_str(rng.pick(tokens));
        }
        let hostile = if config.raw_values {
            hostile
        } else {
            hostile.trim_matches(EDGE).to_string()
        };
        for container in CONTAINERS {
            for (suite, value, tally) in [
                ("G", &value, &mut prose_tally),
                ("H", &hostile, &mut hostile_tally),
            ] {
                let mut tree = in_container(container, value);
                normalize(&mut tree, config);
                let outcome = check(&tree, Grammar::Gfm, config);
                let entry = tally.entry((*container).to_string()).or_default();
                entry.0 += 1;
                if outcome.kind == "equal" {
                    entry.1 += 1;
                }
                if outcome.pd_equal {
                    entry.2 += 1;
                }
                let stage6 = STAGE6_EVENTS.with(|flag| *flag.borrow());
                if outcome.kind != "equal" || (stage6 && !outcome.pd_equal) {
                    records.push(Record {
                        suite,
                        name: format!("{container}: {value:?}"),
                        input: value.clone(),
                        outcome,
                        html: false,
                        note: "",
                    });
                }
            }
        }
    }
    let mut random_docs = BTreeMap::new();
    let mut pulldown_source_panics = 0usize;
    for (suite, first) in [("J", Grammar::Gfm), ("K", Grammar::Dialect)] {
        let (mut total, mut equal, mut pd_equal) = (0usize, 0usize, 0usize);
        for _ in 0..DOCS {
            let source = random_source(&mut rng, DOC_TOKENS, 24);
            // Can pulldown-cmark read this source at all? That's Stage 6's job for a Markdown Input.
            if guarded(|| Parser::new_ext(&source, flags(Grammar::Gfm)).count()).is_err() {
                pulldown_source_panics += 1;
            }
            let Some(record) = from_source(suite, format!("doc {source:?}"), &source, first, Grammar::Gfm, config) else {
                continue;
            };
            total += 1;
            equal += usize::from(record.outcome.kind == "equal");
            pd_equal += usize::from(record.outcome.pd_equal);
            if record.outcome.kind != "equal" || !record.outcome.pd_equal {
                records.push(record);
            }
        }
        random_docs.insert(suite, (total, equal, pd_equal));
    }

    let mut summary = format!(
        "label: {label}\nconfig: literal_autolinks={} spread_rule={}\nPRD example exact: {prd_exact}\n\n",
        config.literal_autolinks, config.spread_rule
    );
    if !prd_exact {
        let _ = writeln!(summary, "PRD output was:\n{prd}\n");
    }
    let mut misses = String::new();
    let mut outputs = String::new();
    for suite in ["A", "B", "P", "C", "D", "E", "F"] {
        let in_suite: Vec<&Record> = records.iter().filter(|r| r.suite == suite).collect();
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for record in &in_suite {
            *kinds.entry(record.outcome.kind).or_default() += 1;
        }
        let equal = kinds.get("equal").copied().unwrap_or(0);
        let pd = in_suite.iter().filter(|r| r.outcome.pd_equal).count();
        let html_misses = in_suite.iter().filter(|r| r.outcome.kind != "equal" && r.html).count();
        let n = in_suite.len();
        let _ = writeln!(
            summary,
            "suite {suite}: {equal}/{n} equal ({:.2}%) {kinds:?}; html misses {html_misses}; under pulldown-cmark {pd}/{n} ({:.2}%)",
            100.0 * equal as f64 / n as f64,
            100.0 * pd as f64 / n as f64
        );
    }
    let unexpected = records.iter().filter(|r| r.note == "UNEXPECTED").count();
    let _ = writeln!(summary, "suite F unexpected: {unexpected}");

    let _ = writeln!(summary, "\nrandom values, {VALUES} per container:");
    for (name, tally) in [("G prose", &prose_tally), ("H hostile", &hostile_tally)] {
        for (container, (n, equal, pd)) in tally {
            let _ = writeln!(summary, "  {name:9} {container:16} {equal}/{n} ({:.2}%)  pulldown {pd}/{n}", 100.0 * *equal as f64 / *n as f64);
        }
    }
    let _ = writeln!(summary, "\nrandom documents:");
    for (suite, (total, equal, pd)) in &random_docs {
        let name = if *suite == "J" { "J GFM" } else { "K Dialect" };
        let _ = writeln!(summary, "  {name:9} {equal}/{total} ({:.3}%)  pulldown {pd}/{total} ({:.3}%)", 100.0 * *equal as f64 / *total as f64, 100.0 * *pd as f64 / *total as f64);
    }
    let mut doc_kinds: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for record in records.iter().filter(|r| matches!(r.suite, "J" | "K")) {
        *doc_kinds.entry((record.suite, record.outcome.kind)).or_default() += 1;
    }
    let _ = writeln!(summary, "  kinds of miss: {doc_kinds:?}");
    let panics = records.iter().filter(|r| r.outcome.kind == "panic").count();
    let serializer_panics = records
        .iter()
        .filter(|r| r.outcome.kind == "panic" && !r.outcome.detail.contains("parse:"))
        .count();
    let _ = writeln!(
        summary,
        "\npanics, all suites: {panics} ({serializer_panics} in the serializer, the rest in the markdown parser)"
    );
    let _ = writeln!(
        summary,
        "pulldown-cmark panics: {} reading serialized output, {pulldown_source_panics} reading the random source documents",
        PULLDOWN_PANICS.with(|count| *count.borrow())
    );

    for record in &records {
        if matches!(record.suite, "C" | "D" | "E" | "F") {
            let _ = writeln!(
                outputs,
                "{} | {} {} | {} | {:?}",
                record.suite, record.outcome.kind, record.note, record.name, record.outcome.serialized
            );
        }
        let keep = record.outcome.kind != "equal" || (!record.outcome.pd_equal && matches!(record.suite, "A" | "B" | "C" | "D" | "E" | "F" | "P" | "J" | "K" | "G" | "H"));
        if keep {
            let _ = write!(
                misses,
                "=== {} | {} {} | pd_equal={} | html={} | {}\ninput: {:?}\nserialized: {:?}\n{}\n\n",
                record.suite, record.outcome.kind, record.note, record.outcome.pd_equal, record.html, record.name,
                record.input, record.outcome.serialized, record.outcome.detail
            );
        }
    }

    let out = manifest.join("results").join(label);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("summary.txt"), &summary).unwrap();
    std::fs::write(out.join("misses.txt"), &misses).unwrap();
    std::fs::write(out.join("outputs.txt"), &outputs).unwrap();
    print!("{summary}");
}
