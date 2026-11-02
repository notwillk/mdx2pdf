//! Spike 2: does `markdown` give us the tree the design needs?
//!
//! Usage:
//!   spike-dialect run <label>       Deterministic suites, into results/<label>/
//!   spike-dialect deep <label>      Depth and scaling in child processes (timings vary)
//!   spike-dialect one <input...>    Prints the outcome for each input, unguarded by state
//!   spike-dialect child ...         Internal: one depth or scaling measurement

mod cases;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use markdown::mdast::{AttributeContent, AttributeValue, Node};
use markdown::message::{Message, Place};
use markdown::unist::Position;
use markdown::{to_mdast, Constructs, ParseOptions};

thread_local! {
    static PANIC_LOCATION: RefCell<String> = const { RefCell::new(String::new()) };
}

fn dialect() -> ParseOptions {
    ParseOptions {
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
            gfm_autolink_literal: false,
            ..Constructs::gfm()
        },
        ..ParseOptions::gfm()
    }
}

fn gfm_no_literal() -> ParseOptions {
    ParseOptions {
        constructs: Constructs {
            gfm_autolink_literal: false,
            ..Constructs::gfm()
        },
        ..ParseOptions::gfm()
    }
}

/// Brace and string aware: the `}` only ends the Expression outside a Liquid string.
fn liquid_expression(value: &str, _kind: &markdown::MdxExpressionKind) -> markdown::MdxSignal {
    use markdown::MdxSignal as Signal;
    let mut quote = None;
    let mut depth = 0usize;
    for c in value.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '{') => depth += 1,
            (None, '}') => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    if quote.is_some() || depth > 0 {
        Signal::Eof("Unexpected end of file in expression".into(), Box::new("spike".into()), Box::new("expr".into()))
    } else {
        Signal::Ok
    }
}

fn dialect_liquid() -> ParseOptions {
    ParseOptions {
        mdx_expression_parse: Some(Box::new(liquid_expression)),
        ..dialect()
    }
}

fn dialect_esm_rejecting() -> ParseOptions {
    use markdown::MdxSignal as Signal;
    let mut options = dialect();
    options.constructs.mdx_esm = true;
    options.mdx_esm_parse = Some(Box::new(|_: &str| {
        Signal::Error("ESM is not supported".into(), 0, Box::new("spike".into()), Box::new("esm".into()))
    }));
    options
}

enum Outcome {
    Tree(Node),
    Error(Message),
    Panic(String),
}

fn parse(input: &str, options: &ParseOptions) -> Outcome {
    match catch_unwind(AssertUnwindSafe(|| to_mdast(input, options))) {
        Ok(Ok(tree)) => Outcome::Tree(tree),
        Ok(Err(message)) => Outcome::Error(message),
        Err(_) => Outcome::Panic(PANIC_LOCATION.with(|l| l.borrow().clone())),
    }
}

// ---------------------------------------------------------------------------
// Compact trees
// ---------------------------------------------------------------------------

fn attributes(list: &[AttributeContent]) -> String {
    let mut out = String::new();
    for attribute in list {
        out.push(' ');
        match attribute {
            AttributeContent::Expression(e) => {
                let _ = write!(out, "{{{}}}", e.value);
            }
            AttributeContent::Property(p) => {
                out.push_str(&p.name);
                match &p.value {
                    None => {}
                    Some(AttributeValue::Literal(l)) => {
                        let _ = write!(out, "=\"{l}\"");
                    }
                    Some(AttributeValue::Expression(e)) => {
                        let _ = write!(out, "={{{}}}", e.value);
                    }
                }
            }
        }
    }
    out
}

fn compact(node: &Node) -> String {
    let group = |name: &str, children: &[Node]| {
        if children.is_empty() {
            name.to_string()
        } else {
            format!("{name}[{}]", children.iter().map(compact).collect::<Vec<_>>().join(","))
        }
    };
    match node {
        Node::Root(n) => group("Root", &n.children),
        Node::Paragraph(n) => group("P", &n.children),
        Node::Heading(n) => group(&format!("H{}", n.depth), &n.children),
        Node::Blockquote(n) => group("Q", &n.children),
        Node::List(n) => group(if n.ordered { "OL" } else { "UL" }, &n.children),
        Node::ListItem(n) => group("LI", &n.children),
        Node::Table(n) => group("Table", &n.children),
        Node::TableRow(n) => group("Row", &n.children),
        Node::TableCell(n) => group("Cell", &n.children),
        Node::Emphasis(n) => group("Em", &n.children),
        Node::Strong(n) => group("Strong", &n.children),
        Node::Delete(n) => group("Del", &n.children),
        Node::Link(n) => group(&format!("Link({})", n.url), &n.children),
        Node::Text(n) => format!("T({:?})", n.value),
        Node::InlineCode(n) => format!("ICode({:?})", n.value),
        Node::Code(n) => format!("Code({:?})", n.value),
        Node::MdxFlowExpression(n) => format!("ExprF({:?})", n.value),
        Node::MdxTextExpression(n) => format!("ExprT({:?})", n.value),
        Node::MdxJsxFlowElement(n) => {
            let name = n.name.clone().unwrap_or_else(|| "<>".into());
            group(&format!("JsxF({name}{})", attributes(&n.attributes)), &n.children)
        }
        Node::MdxJsxTextElement(n) => {
            let name = n.name.clone().unwrap_or_else(|| "<>".into());
            group(&format!("JsxT({name}{})", attributes(&n.attributes)), &n.children)
        }
        other => format!("{other:?}").split([' ', '(']).next().unwrap_or("?").to_string(),
    }
}

fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Tree(tree) => compact(tree),
        Outcome::Error(message) => format!("Err: {message}"),
        Outcome::Panic(location) => format!("PANIC at {location}"),
    }
}

// ---------------------------------------------------------------------------
// The validator a real Stage 3 would need
// ---------------------------------------------------------------------------

#[derive(PartialEq)]
enum Content {
    Flow,
    Phrasing,
    Items,
    Rows,
    Cells,
    Leaf,
}

fn content_of(node: &Node) -> Content {
    match node {
        Node::Root(_) | Node::Blockquote(_) | Node::ListItem(_) | Node::FootnoteDefinition(_) | Node::MdxJsxFlowElement(_) => Content::Flow,
        Node::Paragraph(_) | Node::Heading(_) | Node::Emphasis(_) | Node::Strong(_) | Node::Delete(_) | Node::Link(_) | Node::LinkReference(_) | Node::TableCell(_) | Node::MdxJsxTextElement(_) => Content::Phrasing,
        Node::List(_) => Content::Items,
        Node::Table(_) => Content::Rows,
        Node::TableRow(_) => Content::Cells,
        _ => Content::Leaf,
    }
}

fn is_flow(node: &Node) -> bool {
    matches!(
        node,
        Node::Blockquote(_) | Node::Code(_) | Node::Definition(_) | Node::FootnoteDefinition(_) | Node::Heading(_) | Node::List(_) | Node::MdxFlowExpression(_) | Node::MdxJsxFlowElement(_) | Node::Paragraph(_) | Node::Table(_) | Node::ThematicBreak(_)
    )
}

fn is_phrasing(node: &Node) -> bool {
    matches!(
        node,
        Node::Break(_) | Node::Delete(_) | Node::Emphasis(_) | Node::FootnoteReference(_) | Node::Image(_) | Node::ImageReference(_) | Node::InlineCode(_) | Node::Link(_) | Node::LinkReference(_) | Node::MdxJsxTextElement(_) | Node::MdxTextExpression(_) | Node::Strong(_) | Node::Text(_)
    )
}

fn kind(node: &Node) -> String {
    format!("{node:?}").split([' ', '(']).next().unwrap_or("?").to_string()
}

/// A child in the wrong kind of parent. Returns the first problem.
fn shape_problem(node: &Node) -> Option<String> {
    let children = node.children()?;
    let content = content_of(node);
    for child in children {
        let ok = match content {
            Content::Flow => is_flow(child),
            Content::Phrasing => is_phrasing(child),
            Content::Items => matches!(child, Node::ListItem(_)),
            Content::Rows => matches!(child, Node::TableRow(_)),
            Content::Cells => matches!(child, Node::TableCell(_)),
            Content::Leaf => false,
        };
        if !ok {
            return Some(format!("{} inside {}", kind(child), kind(node)));
        }
    }
    children.iter().find_map(shape_problem)
}

/// Positions: present, ordered, inside the parent, and inside the input.
fn position_problem(node: &Node, parent: Option<&Position>, length: usize) -> Option<String> {
    let Some(position) = node.position() else {
        return Some(format!("{} has no position", kind(node)));
    };
    if position.start.offset > position.end.offset || position.end.offset > length {
        return Some(format!("{} has a bad range", kind(node)));
    }
    if let Some(parent) = parent {
        if position.start.offset < parent.start.offset || position.end.offset > parent.end.offset {
            return Some(format!("{} escapes its parent", kind(node)));
        }
    }
    let children = node.children()?;
    let mut previous = position.start.offset;
    for child in children {
        if let Some(p) = child.position() {
            if p.start.offset < previous {
                return Some(format!("{} starts before its sibling ends", kind(child)));
            }
            previous = p.end.offset;
        }
        if let Some(problem) = position_problem(child, Some(position), length) {
            return Some(problem);
        }
    }
    None
}

fn strip(node: &mut Node) {
    node.position_set(None);
    if let Some(children) = node.children_mut() {
        children.iter_mut().for_each(strip);
    }
}

// ---------------------------------------------------------------------------
// ESM detectors
// ---------------------------------------------------------------------------

/// Naive: a top-level paragraph at column 1 starting with the keyword.
fn esm_line_check(tree: &Node) -> bool {
    tree.children().is_some_and(|children| {
        children.iter().any(|node| {
            let Node::Paragraph(p) = node else { return false };
            let at_start = p.position.as_ref().is_some_and(|x| x.start.column == 1);
            let Some(Node::Text(text)) = p.children.first() else { return false };
            at_start && (text.value.starts_with("import ") || text.value.starts_with("export "))
        })
    })
}

/// Refined: the same place, but the rest of the line has to look like a statement.
///
/// Reads the source line, because `import {a} from 'b'` parses `{a}` as an
/// Expression, and an indented line starts with a space and so doesn't match.
fn esm_refined_check(tree: &Node, input: &str) -> bool {
    let looks_like_esm = |line: &str| {
        let Some((word, rest)) = line.split_once(' ') else { return false };
        let rest = rest.trim();
        match word {
            "import" => {
                rest.starts_with(['"', '\'', '{', '*'])
                    || (rest.split_whitespace().nth(1) == Some("from"))
                    || rest.contains(" from ")
            }
            "export" => {
                ["const ", "let ", "var ", "function ", "class ", "default ", "async "].iter().any(|k| rest.starts_with(k))
                    || rest.starts_with(['{', '*'])
            }
            _ => false,
        }
    };
    tree.children().is_some_and(|children| {
        children.iter().any(|node| {
            let Node::Paragraph(p) = node else { return false };
            let Some(position) = p.position.as_ref().filter(|x| x.start.column == 1) else { return false };
            looks_like_esm(input[position.start.offset..].lines().next().unwrap_or(""))
        })
    })
}

// ---------------------------------------------------------------------------
// Random input
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

fn random_source(rng: &mut Rng, limit: usize) -> String {
    let count = 1 + rng.below(limit);
    (0..count).map(|_| rng.pick(cases::TOKENS)).collect()
}

fn mutate(rng: &mut Rng, input: &str) -> String {
    let mut chars: Vec<char> = input.chars().collect();
    for _ in 0..1 + rng.below(3) {
        let at = rng.below(chars.len() + 1);
        match rng.below(3) {
            0 if at < chars.len() => {
                chars.remove(at);
            }
            1 => {
                let token: Vec<char> = rng.pick(cases::TOKENS).chars().collect();
                chars.splice(at..at, token);
            }
            _ if at < chars.len() => {
                let token = rng.pick(cases::TOKENS).chars().next().unwrap_or('x');
                chars[at] = token;
            }
            _ => {}
        }
    }
    chars.into_iter().collect()
}

/// Removes characters while the same line keeps panicking.
fn reduce(input: &str, location: &str, options: &ParseOptions) -> String {
    let same = |text: &str| matches!(parse(text, options), Outcome::Panic(at) if at == location);
    let mut chars: Vec<char> = input.chars().collect();
    let mut size = chars.len().max(2) / 2;
    while size >= 1 {
        let mut start = 0;
        while start + size <= chars.len() {
            let mut candidate = chars.clone();
            candidate.drain(start..start + size);
            let text: String = candidate.iter().collect();
            if same(&text) {
                chars = candidate;
            } else {
                start += 1;
            }
        }
        size /= 2;
    }
    chars.into_iter().collect()
}

fn reduce_shape(input: &str, wanted: &str, options: &ParseOptions) -> String {
    let same = |text: &str| match parse(text, options) {
        Outcome::Tree(tree) => shape_problem(&tree).as_deref() == Some(wanted),
        _ => false,
    };
    let mut chars: Vec<char> = input.chars().collect();
    let mut size = chars.len().max(2) / 2;
    while size >= 1 {
        let mut start = 0;
        while start + size <= chars.len() {
            let mut candidate = chars.clone();
            candidate.drain(start..start + size);
            let text: String = candidate.iter().collect();
            if same(&text) {
                chars = candidate;
            } else {
                start += 1;
            }
        }
        size /= 2;
    }
    chars.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

fn controls() {
    let tree = |text: &str| match parse(text, &dialect()) {
        Outcome::Tree(tree) => tree,
        other => panic!("control: {text:?} gave {}", describe(&other)),
    };
    assert_ne!(compact(&tree("a\n")), compact(&tree("b\n")), "control: printer must see text");
    assert_ne!(compact(&tree("<A />\n")), compact(&tree("<B />\n")), "control: printer must see names");
    assert_ne!(compact(&tree("<A b={1} />\n")), compact(&tree("<A b=\"1\" />\n")), "control: printer must see attribute kinds");

    assert!(catch_unwind(|| panic!("deliberate")).is_err(), "control: the guard must catch a panic");

    let good = tree("# a\n\n- b\n\n> c *d*\n\n| e |\n|---|\n| f |\n\n<A>\nx\n</A>\n");
    assert_eq!(shape_problem(&good), None, "control: a good tree must validate");
    let mut bad = good.clone();
    if let Some(children) = bad.children_mut() {
        let list_position = children.iter().position(|n| matches!(n, Node::List(_))).unwrap();
        let list = children[list_position].clone();
        if let Node::List(outer) = &mut children[list_position] {
            outer.children.push(list);
        }
    }
    assert!(shape_problem(&bad).is_some(), "control: a list directly in a list must fail validation");

    let outcome = run_child(&["child", "abort"], Duration::from_secs(10));
    assert!(outcome.starts_with("abort"), "control: an aborting child must be reported, got {outcome}");
    let outcome = run_child(&["child", "overflow"], Duration::from_secs(30));
    assert!(outcome.starts_with("abort"), "control: a stack overflow must be reported, got {outcome}");
}

// ---------------------------------------------------------------------------
// The deterministic suites
// ---------------------------------------------------------------------------

fn run_suites(label: &str) {
    controls();
    let mut summary = format!("label: {label}\n\n");
    let mut detail = String::new();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));

    // Table suite.
    let mut surprises = Vec::new();
    let mut groups: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let _ = writeln!(detail, "# Table suite\n");
    for (group, input, expected) in cases::CASES {
        let outcome = parse(input, &dialect());
        let actual = describe(&outcome);
        let matched = match (&outcome, *expected) {
            (Outcome::Error(_), "Err") => true,
            (Outcome::Tree(tree), expected) => compact(tree) == expected,
            _ => false,
        };
        let entry = groups.entry(group).or_default();
        entry.0 += 1;
        if matched {
            entry.1 += 1;
        } else {
            surprises.push((*group, *input, *expected, actual.clone()));
        }
        let mut position = String::new();
        if let Outcome::Tree(tree) = &outcome {
            if let Some(problem) = shape_problem(tree) {
                let _ = write!(position, " SHAPE: {problem}");
            }
            if let Some(problem) = position_problem(tree, None, input.len()) {
                let _ = write!(position, " POSITION: {problem}");
            }
        }
        let _ = writeln!(detail, "[{group}] {input:?}\n  got: {actual}{position}\n  {}", if matched { "as expected".to_string() } else { format!("SURPRISE, expected: {expected}") });
    }
    let _ = writeln!(summary, "table: {} cases, {} surprises", cases::CASES.len(), surprises.len());
    for (group, (total, matched)) in &groups {
        let _ = writeln!(summary, "  {group}: {matched}/{total}");
    }
    for (group, input, expected, actual) in &surprises {
        let _ = writeln!(summary, "  SURPRISE [{group}] {input:?}\n    expected {expected}\n    actual   {actual}");
    }

    // Do tag and Expression nodes point at their source?
    let mut slices = 0;
    let mut wrong_slices = Vec::new();
    fn walk(node: &Node, input: &str, slices: &mut usize, wrong: &mut Vec<String>) {
        let expected_start = match node {
            Node::MdxJsxFlowElement(_) | Node::MdxJsxTextElement(_) => Some('<'),
            Node::MdxFlowExpression(_) | Node::MdxTextExpression(_) => Some('{'),
            _ => None,
        };
        if let (Some(c), Some(p)) = (expected_start, node.position()) {
            *slices += 1;
            if !input[p.start.offset..].starts_with(c) {
                wrong.push(format!("{input:?}: {} at offset {}", kind(node), p.start.offset));
            }
        }
        if let Some(children) = node.children() {
            children.iter().for_each(|child| walk(child, input, slices, wrong));
        }
    }
    let mut errors_with_place = 0;
    let mut errors = 0;
    for (_, input, _) in cases::CASES {
        match parse(input, &dialect()) {
            Outcome::Tree(tree) => walk(&tree, input, &mut slices, &mut wrong_slices),
            Outcome::Error(message) => {
                errors += 1;
                if message.place.is_some() {
                    errors_with_place += 1;
                }
            }
            Outcome::Panic(_) => {}
        }
    }
    let _ = writeln!(summary, "\npositions: {} of {slices} tag and Expression nodes start at their `<` or `{{`", slices - wrong_slices.len());
    for wrong in &wrong_slices {
        let _ = writeln!(summary, "  wrong: {wrong}");
    }
    let _ = writeln!(summary, "error places: {errors_with_place} of {errors} errors carry a location");

    // Expressions with a Liquid-aware callback.
    let _ = writeln!(detail, "\n# Brace counting versus the Liquid callback\n");
    let mut callback_changes = Vec::new();
    for (_, input, _) in cases::CASES {
        let a = describe(&parse(input, &dialect()));
        let b = describe(&parse(input, &dialect_liquid()));
        if a != b {
            callback_changes.push((input.to_string(), a, b));
        }
    }
    let extra = [
        "{x | replace: \"}\", \"\"}\n",
        "a {x | replace: \"}\", \"\"} b\n",
        "{x | replace: '}', ''}\n",
        "{x | replace: \"{\", \"\"}\n",
        "<A b={x | replace: \"}\", \"\"} />\n",
        "<A b={x | replace: \"}\", \"\"}>\ny\n</A>\n",
        "{it's}\n",
        "{\"unclosed}\n",
        "{a\n\"b}\"\nc}\n",
        "| a |\n|---|\n| {x \\| replace: \"}\", \"\"} |\n",
    ];
    let _ = writeln!(summary, "\nliquid callback:");
    for input in extra {
        let _ = writeln!(summary, "  {input:?}\n    braces:   {}\n    callback: {}", describe(&parse(input, &dialect())), describe(&parse(input, &dialect_liquid())));
    }
    let _ = writeln!(summary, "  table cases it changes: {}", callback_changes.len());
    for (input, a, b) in &callback_changes {
        let _ = writeln!(summary, "    {input:?}\n      braces:   {a}\n      callback: {b}");
    }

    // ESM detectors.
    let _ = writeln!(summary, "\nesm detectors (R = must report, - = must not):");
    let mut scores = [0usize; 3];
    for (input, should) in cases::ESM {
        let plain = parse(input, &dialect());
        let line = matches!(&plain, Outcome::Tree(t) if esm_line_check(t));
        let refined = matches!(&plain, Outcome::Tree(t) if esm_refined_check(t, input));
        let rejecting = parse(input, &dialect_esm_rejecting());
        let rejected = matches!(&rejecting, Outcome::Error(m) if m.reason.contains("ESM"));
        for (index, got) in [line, refined, rejected].into_iter().enumerate() {
            if got == *should {
                scores[index] += 1;
            }
        }
        let mark = |got: bool| if got == *should { "ok " } else { "BAD" };
        let _ = writeln!(summary, "  {} line:{} refined:{} rejecting:{} {input:?}", if *should { "R" } else { "-" }, mark(line), mark(refined), mark(rejected));
    }
    let _ = writeln!(summary, "  score of {}: line {}, refined {}, rejecting callback {}", cases::ESM.len(), scores[0], scores[1], scores[2]);

    // Hostile inputs, including after a caught panic.
    let _ = writeln!(summary, "\nhostile inputs:");
    for input in cases::HOSTILE {
        let outcome = parse(input, &dialect());
        let mut note = describe(&outcome);
        if let Outcome::Tree(tree) = &outcome {
            if let Some(problem) = shape_problem(tree) {
                let _ = write!(note, "  MALFORMED: {problem}");
            }
        }
        let _ = writeln!(summary, "  {input:?}: {note}");
    }
    let before = describe(&parse("# known\n\n<A>\nx\n</A>\n", &dialect()));
    let mut panics_first = 0;
    for input in cases::HOSTILE {
        if matches!(parse(input, &dialect()), Outcome::Panic(_)) {
            panics_first += 1;
        }
    }
    let after = describe(&parse("# known\n\n<A>\nx\n</A>\n", &dialect()));
    let _ = writeln!(summary, "  known document parses the same after {panics_first} caught panics: {}", before == after);

    // Written after the first run, so there's no expected result: they record behavior.
    let _ = writeln!(summary, "\nprobes:");
    for (group, inputs) in cases::PROBES {
        let _ = writeln!(summary, "  {group}");
        for input in *inputs {
            let _ = writeln!(summary, "    {input:?}: {}", describe(&parse(input, &dialect())));
        }
    }

    // Random documents and mutations.
    robustness(&mut summary, &mut detail);

    // Dialect versus Markdown.
    differences(manifest, &mut summary, &mut detail);

    let out = manifest.join("results").join(label);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("summary.txt"), &summary).unwrap();
    std::fs::write(out.join("detail.txt"), &detail).unwrap();
    print!("{summary}");
}

fn robustness(summary: &mut String, detail: &mut String) {
    #[derive(Default)]
    struct Counts {
        ok: usize,
        err: usize,
        panics: BTreeMap<String, (usize, String)>,
        shapes: BTreeMap<String, (usize, String)>,
        positions: BTreeMap<String, (usize, String)>,
    }
    let measure = |counts: &mut Counts, input: String, options: &ParseOptions| match parse(&input, options) {
        Outcome::Tree(tree) => {
            counts.ok += 1;
            if let Some(problem) = shape_problem(&tree) {
                let entry = counts.shapes.entry(problem).or_insert_with(|| (0, input.clone()));
                entry.0 += 1;
            }
            if let Some(problem) = position_problem(&tree, None, input.len()) {
                let entry = counts.positions.entry(problem).or_insert_with(|| (0, input.clone()));
                entry.0 += 1;
            }
        }
        Outcome::Error(_) => counts.err += 1,
        Outcome::Panic(location) => {
            let entry = counts.panics.entry(location).or_insert_with(|| (0, input.clone()));
            entry.0 += 1;
        }
    };

    let options = dialect();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut random = Counts::default();
    for _ in 0..100_000 {
        let input = random_source(&mut rng, 14);
        measure(&mut random, input, &options);
    }
    let mut mutated = Counts::default();
    for _ in 0..50_000 {
        let (_, base, _) = cases::CASES[rng.below(cases::CASES.len())];
        let input = mutate(&mut rng, base);
        measure(&mut mutated, input, &options);
    }
    // Spike 1b's Markdown-heavy tokens found the malformed trees, which the tag-heavy set doesn't.
    let mut markdown_heavy = Counts::default();
    for _ in 0..100_000 {
        let count = 1 + rng.below(30);
        let input: String = (0..count).map(|_| rng.pick(cases::DOC_TOKENS)).collect();
        measure(&mut markdown_heavy, input, &options);
    }

    for (name, counts) in [("random", &random), ("mutated", &mutated), ("markdown-heavy", &markdown_heavy)] {
        let panics: usize = counts.panics.values().map(|v| v.0).sum();
        let shapes: usize = counts.shapes.values().map(|v| v.0).sum();
        let positions: usize = counts.positions.values().map(|v| v.0).sum();
        let total = counts.ok + counts.err + panics;
        let _ = writeln!(summary, "\n{name}: {total} documents: {} trees, {} errors, {panics} caught panics, {shapes} malformed shapes, {positions} position problems", counts.ok, counts.err);
        for (location, (count, input)) in &counts.panics {
            let small = reduce(input, location, &options);
            let _ = writeln!(summary, "  panic x{count} at {location}: {small:?}");
        }
        for (problem, (count, input)) in &counts.shapes {
            let small = reduce_shape(input, problem, &options);
            let _ = writeln!(summary, "  shape x{count} {problem}: {small:?}");
        }
        for (problem, (count, input)) in &counts.positions {
            let _ = writeln!(summary, "  position x{count} {problem}, first seen in {input:?}");
        }
    }
    let _ = writeln!(detail, "\n(random documents are reproducible from the seed in robustness())");
}

fn differences(manifest: &Path, summary: &mut String, detail: &mut String) {
    let spec: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(manifest.join("../01-serializer/spec.json")).unwrap()).unwrap();
    let spec = spec.as_array().unwrap();
    assert_eq!(spec.len(), 652, "CommonMark 0.31.2 has 652 examples");

    // One change at a time from Markdown, to attribute each difference to a cause.
    type Toggle = (&'static str, fn(&mut Constructs));
    let toggles: [Toggle; 5] = [
        ("html off", |c| {
            c.html_flow = false;
            c.html_text = false;
        }),
        ("indented code off", |c| c.code_indented = false),
        ("autolinks off", |c| c.autolink = false),
        ("jsx on", |c| {
            c.mdx_jsx_flow = true;
            c.mdx_jsx_text = true;
        }),
        ("expressions on", |c| {
            c.mdx_expression_flow = true;
            c.mdx_expression_text = true;
        }),
    ];
    let stripped = |input: &str, options: &ParseOptions| -> Result<String, String> {
        match parse(input, options) {
            Outcome::Tree(mut tree) => {
                strip(&mut tree);
                Ok(format!("{tree:?}"))
            }
            Outcome::Error(message) => Err(message.reason),
            Outcome::Panic(location) => Err(format!("panic at {location}")),
        }
    };

    let mut same = 0;
    let mut different = 0;
    let mut errored = 0;
    let mut causes: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut reasons: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let _ = writeln!(detail, "\n# Dialect versus Markdown\n");
    for example in spec {
        let input = example["markdown"].as_str().unwrap();
        let number = example["example"].as_u64().unwrap();
        let base = stripped(input, &gfm_no_literal());
        let dial = stripped(input, &dialect());
        if base == dial {
            same += 1;
            continue;
        }
        let mut names = Vec::new();
        for (name, toggle) in &toggles {
            let mut options = gfm_no_literal();
            toggle(&mut options.constructs);
            if stripped(input, &options) != base {
                names.push(*name);
            }
        }
        let cause = if names.is_empty() { "only in combination".to_string() } else { names.join(" + ") };
        match &dial {
            Err(reason) => {
                errored += 1;
                let short: String = reason.split('(').next().unwrap_or("").chars().take(90).collect();
                reasons.entry(short).or_default().push(number);
                causes.entry(format!("error: {cause}")).or_default().push(number);
            }
            Ok(_) => {
                different += 1;
                causes.entry(format!("tree: {cause}")).or_default().push(number);
            }
        }
        let _ = writeln!(detail, "example {number} [{cause}] {input:?}\n  dialect: {}", match &dial { Ok(_) => describe(&parse(input, &dialect())), Err(reason) => reason.clone() });
    }
    let _ = writeln!(summary, "\ndialect versus markdown, 652 spec examples: {same} same tree, {different} different tree, {errored} dialect errors");
    for (cause, numbers) in &causes {
        let list: Vec<String> = numbers.iter().take(12).map(u64::to_string).collect();
        let _ = writeln!(summary, "  {:>3} {cause}: {}{}", numbers.len(), list.join(", "), if numbers.len() > 12 { ", ..." } else { "" });
    }
    let _ = writeln!(summary, "dialect error messages:");
    for (reason, numbers) in &reasons {
        let _ = writeln!(summary, "  {:>3} {reason}", numbers.len());
    }
    let place_examples = ["<br>\n", "a < b\n", "<A></B>\n", "text\n\n<A>\n", "a { b\n", "<https://a.b>\n", "<!-- x -->\n"];
    let _ = writeln!(summary, "error text and location:");
    for input in place_examples {
        if let Outcome::Error(message) = parse(input, &dialect()) {
            let place = match message.place.as_deref() {
                Some(Place::Point(p)) => format!("{}:{}", p.line, p.column),
                Some(Place::Position(p)) => format!("{}:{}", p.start.line, p.start.column),
                None => "none".into(),
            };
            let _ = writeln!(summary, "  {input:?} at {place}: {}", message.reason);
        }
    }
}

// ---------------------------------------------------------------------------
// Child processes: depth and scaling
// ---------------------------------------------------------------------------

fn deep_input(shape: &str, depth: usize) -> String {
    match shape {
        "quote" => "> ".repeat(depth) + "x\n",
        "list" => "- ".repeat(depth) + "x\n",
        "ordered" => "1. ".repeat(depth) + "x\n",
        "jsx-text" => format!("{}x{}\n", "<A>".repeat(depth), "</A>".repeat(depth)),
        "jsx-flow" => format!("{}x\n{}", "<A>\n".repeat(depth), "</A>\n".repeat(depth)),
        "emphasis" => format!("{}x{}\n", "*a _b ".repeat(depth), " b_ a*".repeat(depth)),
        "link" => format!("{}x{}\n", "[".repeat(depth), "](/u)".repeat(depth)),
        "brace" => format!("{}{}\n", "{".repeat(depth), "}".repeat(depth)),
        other => panic!("unknown shape {other}"),
    }
}

fn scale_input(shape: &str, bytes: usize) -> String {
    let unit = match shape {
        "prose" => "The quick brown fox jumps over the lazy dog, again and again.\n\n",
        "one-paragraph" => "The quick brown fox jumps over the lazy dog, again and again.\n",
        "tags" => "<A b=\"c\">x</A>\n\n",
        "expressions" => "a {x | y} b\n\n",
        "emphasis-open" => "*a ",
        "brackets" => "[a ",
        "backticks" => "`a ``b ",
        "list" => "- item\n",
        "table" => "| a | b |\n",
        other => panic!("unknown shape {other}"),
    };
    let prefix = if shape == "table" { "| a | b |\n|---|---|\n" } else { "" };
    format!("{prefix}{}", unit.repeat(bytes / unit.len() + 1))
}

fn child(args: &[String]) {
    #[allow(unconditional_recursion)]
    fn overflow(n: u64) -> u64 {
        std::hint::black_box(overflow(std::hint::black_box(n + 1)) + 1)
    }
    match args[0].as_str() {
        "abort" => std::process::abort(),
        "overflow" => {
            println!("{}", overflow(0));
        }
        "deep" | "scale" => {
            let input = if args[0] == "deep" { deep_input(&args[1], args[2].parse().unwrap()) } else { scale_input(&args[1], args[2].parse().unwrap()) };
            let stack: usize = args[3].parse().unwrap();
            let validate = args.get(4).map(String::as_str) == Some("validate");
            let work = move || {
                let start = Instant::now();
                let result = catch_unwind(AssertUnwindSafe(|| to_mdast(&input, &dialect())));
                let parsed = start.elapsed();
                let note = match &result {
                    Ok(Ok(tree)) => {
                        if validate {
                            format!("tree, shape {:?}", shape_problem(tree))
                        } else {
                            "tree".to_string()
                        }
                    }
                    Ok(Err(_)) => "err".to_string(),
                    Err(_) => "caught panic".to_string(),
                };
                drop(result);
                println!("{note} {:.3}s", parsed.as_secs_f64());
            };
            if stack == 0 {
                work();
            } else {
                std::thread::Builder::new().stack_size(stack).spawn(work).unwrap().join().unwrap();
            }
        }
        other => panic!("unknown child {other}"),
    }
}

fn self_path() -> PathBuf {
    std::env::current_exe().unwrap()
}

fn run_child(args: &[&str], timeout: Duration) -> String {
    let mut process = Command::new(self_path()).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let start = Instant::now();
    loop {
        match process.try_wait().unwrap() {
            Some(status) => {
                let output = process.wait_with_output().unwrap();
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                return if status.success() { text } else { format!("abort ({status})") };
            }
            None if start.elapsed() > timeout => {
                let _ = process.kill();
                let _ = process.wait();
                return format!("timeout (over {}s)", timeout.as_secs());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn run_deep(label: &str) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let timeout = Duration::from_secs(20);
    let mut out = format!("label: {label}\nmain-thread default stack first; 1 GiB thread only where that failed. Timeout 20s.\n\n");
    let shapes = ["quote", "list", "ordered", "jsx-text", "jsx-flow", "emphasis", "link", "brace"];
    for shape in shapes {
        for depth in [100usize, 1_000, 10_000, 100_000] {
            let depth_text = depth.to_string();
            let main = run_child(&["child", "deep", shape, &depth_text, "0"], timeout);
            let mut line = format!("{shape:>9} depth {depth:>6}: {main}");
            if main.starts_with("abort") {
                let big = run_child(&["child", "deep", shape, &depth_text, "1073741824"], timeout);
                let _ = write!(line, "   | 1 GiB stack: {big}");
            }
            println!("{line}");
            let _ = writeln!(out, "{line}");
            if main.starts_with("timeout") {
                break;
            }
        }
    }
    let _ = writeln!(out, "\nscaling, parse time by input size (reported only):");
    println!("\nscaling:");
    for shape in ["prose", "one-paragraph", "tags", "expressions", "emphasis-open", "brackets", "backticks", "list", "table"] {
        let mut line = format!("{shape:>14}:");
        for kib in [16usize, 64, 256, 1024] {
            let bytes = (kib * 1024).to_string();
            let result = run_child(&["child", "scale", shape, &bytes, "0"], Duration::from_secs(60));
            let _ = write!(line, "  {kib}K {result}");
            if !result.starts_with("tree") && !result.starts_with("err") {
                break;
            }
        }
        println!("{line}");
        let _ = writeln!(out, "{line}");
    }
    let dir = manifest.join("results").join(label);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("deep.txt"), out).unwrap();
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map_or(String::new(), |l| format!("{}:{}", l.file().rsplit('/').next().unwrap_or(""), l.line()));
        PANIC_LOCATION.with(|l| *l.borrow_mut() = location);
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => run_suites(&args[1]),
        Some("deep") => run_deep(&args[1]),
        Some("child") => child(&args[1..]),
        Some("one") => {
            for input in &args[1..] {
                let input = input.replace("\\n", "\n");
                let outcome = parse(&input, &dialect());
                println!("{input:?}\n  {}", describe(&outcome));
            }
        }
        _ => eprintln!("usage: spike-dialect run|deep <label>"),
    }
}
