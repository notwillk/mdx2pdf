//! The smallest version of the technical design's `instantiate`, from a Project to
//! Intermediate Markdown: stages 1 to 5, with the checks from Spikes 2 and 3.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use liquid::model::Value;
use liquid_core::ValueView;
use markdown::mdast::{AttributeContent, AttributeValue, Node, Paragraph, Root, Text};
use markdown::message::{Message, Place};
use markdown::unist::Position;
use markdown::{to_mdast, Constructs, MdxSignal, ParseOptions};

use crate::serialize::{canonical, first_difference, localize_references, normalize, parse_markdown, serialize, to_events};
use crate::template::{capture, has_delimiter_outside_string, render, tabs_to_spaces_outside_strings, Engines, Failure, EDGE};

/// Well above what Typst can lay out, and far below where a recursive walk overflows.
const MAX_DEPTH: usize = 100;

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

pub trait Files {
    /// `None` if there's no such file, and an error if it isn't text.
    fn read(&self, path: &str) -> Option<Result<String, String>>;
}

pub struct Disk(pub PathBuf);

impl Files for Disk {
    fn read(&self, path: &str) -> Option<Result<String, String>> {
        let bytes = std::fs::read(self.0.join(path)).ok()?;
        Some(String::from_utf8(bytes).map_err(|_| "This file isn't valid UTF-8.".to_string()))
    }
}

pub struct Memory(pub BTreeMap<String, String>);

impl Files for Memory {
    fn read(&self, path: &str) -> Option<Result<String, String>> {
        self.0.get(path).cloned().map(Ok)
    }
}

// ---------------------------------------------------------------------------
// Problems
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Where {
    File,
    Source { line: usize, column: Option<usize> },
    Rendered { line: usize, snippet: String },
}

#[derive(Debug, Clone)]
pub struct Problem {
    pub file: String,
    pub at: Where,
    pub message: String,
    pub hint: Option<String>,
    /// Our bug or a parser's, rather than a mistake in the document.
    pub internal: bool,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.at {
            Where::File => write!(f, "{}: {}", self.file, self.message)?,
            Where::Source { line, column: Some(column) } => write!(f, "{}:{line}:{column}: {}", self.file, self.message)?,
            Where::Source { line, column: None } => write!(f, "{}:{line}: {}", self.file, self.message)?,
            Where::Rendered { line, snippet } => {
                write!(f, "{}, after the Liquid Pass, line {line}: {}\n    {snippet}", self.file, self.message)?;
            }
        }
        if let Some(hint) = &self.hint {
            write!(f, "\nhint: {hint}")?;
        }
        Ok(())
    }
}

const FROM_LIQUID: &str = "A value from `{{ }}` is read as MDX. Print data with `{x}`, or with `{{ x | text }}` in a loop, and with `{{ x | attr }}` in a quoted attribute.";

// ---------------------------------------------------------------------------
// Stage 1: a file's text
// ---------------------------------------------------------------------------

struct Source {
    name: String,
    body: String,
    /// Lines before the body, so that line 1 of the body is `offset + 1` in the file.
    offset: usize,
    rendered: String,
}

impl Source {
    fn problem(&self, at: Where, message: impl Into<String>, hint: Option<String>) -> Problem {
        Problem { file: self.name.clone(), at, message: message.into(), hint, internal: false }
    }

    /// A line in the Liquid output, as a place to show. If the text survived unchanged we can
    /// name the source line, and otherwise the rendered snippet has to do.
    fn locate(&self, line: usize, column: Option<usize>) -> Where {
        if self.rendered == self.body {
            return Where::Source { line: line + self.offset, column };
        }
        let snippet = self.rendered.lines().nth(line.saturating_sub(1)).unwrap_or("");
        let matches: Vec<usize> = self.body.lines().enumerate().filter(|(_, l)| !l.trim().is_empty() && *l == snippet).map(|(i, _)| i + 1).collect();
        match matches.as_slice() {
            [only] => Where::Source { line: only + self.offset, column },
            _ => Where::Rendered { line, snippet: snippet.to_string() },
        }
    }

    fn locate_position(&self, position: Option<&Position>) -> Where {
        position.map_or(Where::File, |p| self.locate(p.start.line, Some(p.start.column)))
    }

    fn line_in_rendered(&self, line: usize) -> &str {
        self.rendered.lines().nth(line.saturating_sub(1)).unwrap_or("")
    }

    /// Lines in the source containing a word, for errors with no location of their own.
    fn lines_with(&self, word: &str) -> Vec<usize> {
        self.body.lines().enumerate().filter(|(_, l)| l.contains(word)).map(|(i, _)| i + 1 + self.offset).collect()
    }
}

/// Returns the YAML, the body, and the number of lines before the body.
fn split_frontmatter(name: &str, raw: &str) -> Result<(String, String, usize), Problem> {
    let mut text = raw.strip_prefix('\u{feff}').unwrap_or(raw).replace("\r\n", "\n").replace('\r', "\n");
    // The fork's one known panic needs a file that ends right after the `<`.
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let Some(rest) = text.strip_prefix("---\n") else {
        return Ok((String::new(), text, 0));
    };
    let mut yaml = String::new();
    let mut remaining = rest;
    let mut lines = 1;
    while !remaining.is_empty() {
        let (line, after) = remaining.split_once('\n').unwrap_or((remaining, ""));
        lines += 1;
        if line.trim_end() == "---" {
            return Ok((yaml, after.to_string(), lines));
        }
        yaml.push_str(line);
        yaml.push('\n');
        remaining = after;
    }
    Err(Problem {
        file: name.to_string(),
        at: Where::Source { line: 1, column: None },
        message: "This file starts a frontmatter block with `---`, but never closes it.".into(),
        hint: Some("Close it with a `---` line. To start a document with a thematic break, write `***`.".into()),
        internal: false,
    })
}

/// YAML to a Liquid value. `liquid`'s own `Value` is an untagged enum, which reads every key
/// nested below the top level as a typed scalar, so `n:`, `on:`, and `2024:` fail with "data
/// did not match any variant". Here a key is always text.
struct Data(Value);

impl<'de> serde::Deserialize<'de> for Data {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(DataVisitor)
    }
}

struct DataVisitor;

impl<'de> serde::de::Visitor<'de> for DataVisitor {
    type Value = Data;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a YAML value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Data, E> {
        Ok(Data(Value::scalar(v)))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Data, E> {
        Ok(Data(Value::scalar(v)))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Data, E> {
        Ok(Data(i64::try_from(v).map_or_else(|_| Value::scalar(v as f64), Value::scalar)))
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Data, E> {
        if v.is_finite() {
            Ok(Data(Value::scalar(v)))
        } else {
            Err(E::custom("`.inf` and `.nan` aren't available in frontmatter"))
        }
    }

    fn visit_str<E>(self, v: &str) -> Result<Data, E> {
        Ok(Data(Value::scalar(v.to_string())))
    }

    fn visit_unit<E>(self) -> Result<Data, E> {
        Ok(Data(Value::Nil))
    }

    fn visit_none<E>(self) -> Result<Data, E> {
        Ok(Data(Value::Nil))
    }

    fn visit_some<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Data, D::Error> {
        deserializer.deserialize_any(DataVisitor)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Data, A::Error> {
        let mut out = Vec::new();
        while let Some(Data(value)) = seq.next_element()? {
            out.push(value);
        }
        Ok(Data(Value::Array(out)))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Data, A::Error> {
        let mut out = liquid::Object::new();
        while let Some(key) = map.next_key::<String>()? {
            let Data(value) = map.next_value()?;
            out.insert(key.into(), value);
        }
        Ok(Data(Value::Object(out)))
    }
}

fn parse_frontmatter(name: &str, yaml: &str) -> Result<liquid::Object, Problem> {
    let problem = |message: String| Problem { file: name.to_string(), at: Where::File, message: format!("Couldn't read the frontmatter: {message}"), hint: None, internal: false };
    if yaml.trim().is_empty() {
        return Ok(liquid::Object::new());
    }
    let options = serde_saphyr::options! { strict_booleans: true, };
    match serde_saphyr::from_str_with_options::<Data>(yaml, options) {
        Ok(Data(Value::Object(object))) => Ok(object),
        Ok(Data(Value::Nil)) => Ok(liquid::Object::new()),
        Ok(_) => Err(problem("it has to be a list of `key: value` lines.".to_string())),
        Err(e) => Err(problem(e.to_string().replace('\n', " "))),
    }
}

// ---------------------------------------------------------------------------
// Stage 2: Liquid errors
// ---------------------------------------------------------------------------

fn field<'a>(message: &'a str, key: &str) -> Option<&'a str> {
    message.split(key).nth(1).and_then(|t| t.lines().next()).map(str::trim)
}

fn parse_error_place(message: &str) -> Option<(usize, usize)> {
    let tail = message.split("--> ").nth(1)?;
    let (line, rest) = tail.split_once(':')?;
    let column: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((line.trim().parse().ok()?, column.parse().ok()?))
}

fn parse_error_reason(message: &str) -> String {
    message
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("= "))
        .map_or_else(|| message.lines().next().unwrap_or("").trim_start_matches("liquid: ").trim().to_string(), str::to_string)
}

fn liquid_problem(source: &Source, failure: Failure) -> Problem {
    match failure {
        Failure::Panic => Problem { file: source.name.clone(), at: Where::File, message: "Liquid crashed on this file.".into(), hint: None, internal: true },
        Failure::Parse(message) | Failure::Render(message) => {
            let place = parse_error_place(&message);
            let line_text = place.map_or("", |(line, _)| source.body.lines().nth(line.saturating_sub(1)).unwrap_or(""));
            if let Some(name) = field(&message, "requested variable=") {
                let lines = source.lines_with(name);
                let at = single_line(&lines);
                let hint = if source.body.contains("default:") {
                    "In `liquid`, `default` needs a defined name. Test an optional value with `{% if %}`.".to_string()
                } else {
                    "In code, wrap literal braces in `{% raw %}`. In prose, write `\\{\\{`.".to_string()
                };
                return source.problem(at, format!("`{name}` is undefined."), Some(hint));
            }
            if let (Some(name), Some(variable)) = (field(&message, "requested index="), field(&message, "variable=")) {
                let lines = source.lines_with(name);
                return source.problem(single_line(&lines), format!("`{name}` is undefined in `{variable}`."), None);
            }
            if let Some(name) = field(&message, "requested filter=") {
                let lines = source.lines_with(name);
                return source.problem(single_line(&lines), format!("There's no `{name}` filter."), None);
            }
            let reason = parse_error_reason(&message);
            let at = place.map_or(Where::File, |(line, column)| Where::Source { line: line + source.offset, column: Some(column) });
            let hint = line_text.contains("={{").then(|| "`{{` starts Liquid output here, so a tag can't take an object this way. Pass a value with `name={x}`.".to_string());
            source.problem(at, reason, hint)
        }
    }
}

fn liquid_pass(engines: &Engines, source: &Source, scope: &liquid::Object) -> Result<String, Problem> {
    let mut text = render(&engines.author, &source.body, scope).map_err(|f| liquid_problem(source, f))?;
    if !text.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

fn single_line(lines: &[usize]) -> Where {
    match lines {
        [only] => Where::Source { line: *only, column: None },
        _ => Where::File,
    }
}

// ---------------------------------------------------------------------------
// Stage 3: the Dialect
// ---------------------------------------------------------------------------

fn base_options() -> ParseOptions {
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

/// Brace and string aware: the `}` only ends the Expression outside a Liquid string.
fn liquid_expression(value: &str, _kind: &markdown::MdxExpressionKind) -> MdxSignal {
    if open_quote(value).is_some() || open_braces(value) {
        MdxSignal::Eof("Unexpected end of file in expression".into(), Box::new("mdx2pdf".into()), Box::new("expression".into()))
    } else {
        MdxSignal::Ok
    }
}

fn open_quote(value: &str) -> Option<char> {
    let mut quote = None;
    for c in value.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None => {}
        }
    }
    quote
}

fn open_braces(value: &str) -> bool {
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
    depth > 0
}

fn guarded_parse(text: &str, options: &ParseOptions) -> Result<Result<Node, Message>, ()> {
    catch_unwind(AssertUnwindSafe(|| to_mdast(text, options))).map_err(|_| ())
}

fn message_place(message: &Message) -> Option<(usize, usize)> {
    match message.place.as_deref() {
        Some(Place::Point(p)) => Some((p.line, p.column)),
        Some(Place::Position(p)) => Some((p.start.line, p.start.column)),
        None => None,
    }
}

fn dialect_hint(reason: &str, line: &str, from_liquid: bool) -> Option<String> {
    if from_liquid {
        return Some(FROM_LIQUID.to_string());
    }
    if line.contains("<!--") {
        return Some("Write a comment as `{/* text */}`.".into());
    }
    if line.contains("<http") || line.contains("<mailto") {
        return Some("Write a link as `[text](url)`.".into());
    }
    if line.trim_start().starts_with('|') && (reason.contains("expression") || reason.contains("attribute")) {
        return Some("In a table cell, a filter's pipe needs a backslash: `\\|`.".into());
    }
    if let Some(tag) = reason.split("closing tag for `<").nth(1) {
        return if tag.chars().next().is_some_and(char::is_lowercase) {
            Some("HTML isn't available. Use a Component, and write a literal `<` as `\\<`.".into())
        } else {
            Some("To print a literal `<`, write `\\<`.".into())
        };
    }
    if reason.contains("before name") || reason.contains("name") {
        return Some("To print a literal `<`, write `\\<`.".into());
    }
    None
}

pub fn dialect_options() -> ParseOptions {
    ParseOptions { mdx_expression_parse: Some(Box::new(liquid_expression)), ..base_options() }
}

fn parse_dialect(source: &Source) -> Result<Node, Problem> {
    let text = &source.rendered;
    let with_callback = dialect_options();
    let crashed = || Problem {
        file: source.name.clone(),
        at: Where::File,
        message: "The parser crashed on this file.".into(),
        hint: Some("Check for an unclosed tag or fence. If the file looks right, this is a bug in the parser.".into()),
        internal: true,
    };
    let tree = match guarded_parse(text, &with_callback) {
        Err(()) => return Err(crashed()),
        Ok(Ok(tree)) => tree,
        Ok(Err(message)) => return Err(parse_error(source, &message)),
    };
    run_checks(source, &tree)?;
    Ok(tree)
}

fn parse_error(source: &Source, message: &Message) -> Problem {
    let from_callback = message.source.as_str() == "mdx2pdf";
    if from_callback {
        // The callback's error points at the end of the file. Plain brace matching finds
        // the Expression with the open quote.
        if let Ok(Ok(tree)) = guarded_parse(&source.rendered, &base_options()) {
            if let Some((value, position)) = find_open_quote(&tree) {
                return source.problem(
                    source.locate_position(position.as_ref()),
                    format!("This Expression has a quote with no closing quote: `{{{value}}}`."),
                    Some("A Liquid string needs both quotes. For literal braces, write `\\{`.".into()),
                );
            }
        } else if let Ok(Err(plain)) = guarded_parse(&source.rendered, &base_options()) {
            return plain_parse_error(source, &plain);
        }
    }
    plain_parse_error(source, message)
}

/// `Expected a closing tag for `<br>` (1:1)` points at the end of the file, and names the tag's
/// own place in the text.
fn opening_tag_place(reason: &str) -> Option<(usize, usize)> {
    let tail = reason.split("closing tag for").nth(1)?.split(" (").nth(1)?;
    let (line, rest) = tail.split_once(':')?;
    let column: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((line.parse().ok()?, column.parse().ok()?))
}

fn plain_parse_error(source: &Source, message: &Message) -> Problem {
    let place = opening_tag_place(&message.reason).or_else(|| message_place(message));
    let at = place.map_or(Where::File, |(line, column)| source.locate(line, Some(column)));
    let line = place.map_or("", |(l, _)| source.line_in_rendered(l));
    let hint = dialect_hint(&message.reason, line, matches!(at, Where::Rendered { .. }));
    source.problem(at, message.reason.clone(), hint)
}

fn find_open_quote(node: &Node) -> Option<(String, Option<Position>)> {
    match node {
        Node::MdxFlowExpression(e) if open_quote(&e.value).is_some() => return Some((e.value.clone(), e.position.clone())),
        Node::MdxTextExpression(e) if open_quote(&e.value).is_some() => return Some((e.value.clone(), e.position.clone())),
        _ => {}
    }
    node.children()?.iter().find_map(find_open_quote)
}

fn kind(node: &Node) -> String {
    format!("{node:?}").split([' ', '(']).next().unwrap_or("?").to_string()
}

/// From Spike 2: a child in the wrong kind of parent.
fn shape_problem(node: &Node) -> Option<String> {
    let flow = |n: &Node| matches!(n, Node::Blockquote(_) | Node::Code(_) | Node::Definition(_) | Node::FootnoteDefinition(_) | Node::Heading(_) | Node::List(_) | Node::MdxFlowExpression(_) | Node::MdxJsxFlowElement(_) | Node::Paragraph(_) | Node::Table(_) | Node::ThematicBreak(_));
    let phrasing = |n: &Node| matches!(n, Node::Break(_) | Node::Delete(_) | Node::Emphasis(_) | Node::FootnoteReference(_) | Node::Image(_) | Node::ImageReference(_) | Node::InlineCode(_) | Node::Link(_) | Node::LinkReference(_) | Node::MdxJsxTextElement(_) | Node::MdxTextExpression(_) | Node::Strong(_) | Node::Text(_));
    let children = node.children()?;
    for child in children {
        let ok = match node {
            Node::Root(_) | Node::Blockquote(_) | Node::ListItem(_) | Node::FootnoteDefinition(_) | Node::MdxJsxFlowElement(_) => flow(child),
            Node::Paragraph(_) | Node::Heading(_) | Node::Emphasis(_) | Node::Strong(_) | Node::Delete(_) | Node::Link(_) | Node::LinkReference(_) | Node::TableCell(_) | Node::MdxJsxTextElement(_) => phrasing(child),
            Node::List(_) => matches!(child, Node::ListItem(_)),
            Node::Table(_) => matches!(child, Node::TableRow(_)),
            Node::TableRow(_) => matches!(child, Node::TableCell(_)),
            _ => false,
        };
        if !ok {
            return Some(format!("{} inside {}", kind(child), kind(node)));
        }
    }
    children.iter().find_map(shape_problem)
}

fn depth_of(root: &Node) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(root, 1usize)];
    while let Some((node, depth)) = stack.pop() {
        deepest = deepest.max(depth);
        if let Some(children) = node.children() {
            stack.extend(children.iter().map(|c| (c, depth + 1)));
        }
    }
    deepest
}

fn looks_like_esm(line: &str) -> bool {
    let Some((word, rest)) = line.split_once(' ') else {
        return false;
    };
    let rest = rest.trim();
    match word {
        "import" => {
            rest.starts_with(['"', '\'', '{', '*'])
                || rest.split(" from ").nth(1).is_some_and(|after| after.trim_start().starts_with(['"', '\'']))
        }
        "export" => ["const ", "let ", "var ", "function ", "class ", "default ", "async "].iter().any(|k| rest.starts_with(k)) || rest.starts_with(['{', '*']),
        _ => false,
    }
}

fn is_delimiter_row(line: &str) -> bool {
    let line = line.trim_start_matches([' ', '>', '\t']).trim_end();
    if !line.contains('|') || !line.contains('-') || !line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ' | '\t')) {
        return false;
    }
    line.trim_matches('|').split('|').all(|cell| {
        let cell = cell.trim();
        !cell.is_empty() && cell.trim_matches(':').chars().all(|c| c == '-') && cell.contains('-')
    })
}

/// A `<` and an uppercase letter that the parser left as text, unless escaped. A text node
/// that starts with an escape starts after its backslash, so this looks back past its start.
fn unterminated_tag(text: &str, position: &Option<Position>) -> Option<String> {
    let position = position.as_ref()?;
    let bytes = text.as_bytes();
    for index in position.start.offset..position.end.offset.min(bytes.len().saturating_sub(1)) {
        if bytes[index] == b'<' && bytes[index + 1].is_ascii_uppercase() {
            let backslashes = bytes[..index].iter().rev().take_while(|b| **b == b'\\').count();
            if backslashes % 2 == 0 {
                let name: String = text[index + 1..].chars().take_while(|c| c.is_alphanumeric()).collect();
                return Some(name);
            }
        }
    }
    None
}

fn unescaped_brace(slice: &str) -> bool {
    let bytes = slice.as_bytes();
    bytes.iter().enumerate().any(|(index, b)| *b == b'{' && bytes[..index].iter().rev().take_while(|c| **c == b'\\').count() % 2 == 0)
}

fn run_checks(source: &Source, tree: &Node) -> Result<(), Problem> {
    let depth = depth_of(tree);
    if depth > MAX_DEPTH {
        return Err(source.problem(Where::File, format!("This file is nested {depth} levels deep, and the limit is {MAX_DEPTH}."), None));
    }
    if let Some(problem) = shape_problem(tree) {
        return Err(Problem { file: source.name.clone(), at: Where::File, message: format!("The parser gave a malformed tree: {problem}."), hint: None, internal: true });
    }
    if let Some(children) = tree.children() {
        for node in children {
            let Node::Paragraph(p) = node else { continue };
            let Some(position) = p.position.as_ref().filter(|x| x.start.column == 1) else { continue };
            let line = source.rendered[position.start.offset..].lines().next().unwrap_or("");
            if looks_like_esm(line) {
                let word = line.split(' ').next().unwrap_or("");
                let escape = if word == "import" { "&#105;mport" } else { "&#101;xport" };
                return Err(source.problem(
                    source.locate_position(Some(position)),
                    format!("`{word}` statements aren't available. Liquid is the only logic language."),
                    Some(format!("Use a Component, which needs no `import`. If this is prose, write `{escape}`.")),
                ));
            }
        }
    }
    check_node(source, tree)
}

fn check_node(source: &Source, node: &Node) -> Result<(), Problem> {
    let slice = |position: &Option<Position>| position.as_ref().map_or("", |p| &source.rendered[p.start.offset..p.end.offset]);
    match node {
        Node::MdxFlowExpression(e) if blank_line(&e.value) => return Err(blank_line_problem(source, e.position.as_ref())),
        Node::MdxTextExpression(e) if blank_line(&e.value) => return Err(blank_line_problem(source, e.position.as_ref())),
        Node::Text(t) => {
            if let Some(name) = unterminated_tag(&source.rendered, &t.position) {
                let at = source.locate_position(t.position.as_ref());
                let hint = if matches!(at, Where::Rendered { .. }) { FROM_LIQUID } else { "To print a literal `<`, write `\\<`. Otherwise, finish the tag." };
                return Err(source.problem(at, format!("`<{name}` has no closing `>`, so it would print as text."), Some(hint.to_string())));
            }
        }
        Node::Paragraph(p) => {
            let lines: Vec<&str> = slice(&p.position).lines().collect();
            if lines.windows(2).any(|pair| is_delimiter_row(pair[1]) && pair[0].contains('|')) {
                return Err(source.problem(
                    source.locate_position(p.position.as_ref()),
                    "This looks like a table, but it's a paragraph, because the header and the delimiter row have different numbers of cells.",
                    Some("In a table cell, a filter's pipe needs a backslash: `\\|`.".into()),
                ));
            }
        }
        Node::Link(l) if l.url.contains('{') => {
            return Err(source.problem(
                source.locate_position(l.position.as_ref()),
                format!("`{}` is a link destination, and Expressions don't run there.", l.url),
                Some("Use `{{ url }}`, which runs before the file is read as MDX.".into()),
            ));
        }
        Node::Image(i) => {
            let raw = slice(&i.position);
            let label = raw.split("](").next().unwrap_or("");
            if i.url.contains('{') || unescaped_brace(label) {
                return Err(source.problem(
                    source.locate_position(i.position.as_ref()),
                    "Expressions don't run in an image's alt text or destination.",
                    Some("Use `{{ x | text }}`, which runs before the file is read as MDX.".into()),
                ));
            }
        }
        _ => {}
    }
    if let Some(children) = node.children() {
        for child in children {
            check_node(source, child)?;
        }
    }
    Ok(())
}

fn blank_line(value: &str) -> bool {
    value.lines().skip(1).any(|l| l.trim().is_empty())
}

fn blank_line_problem(source: &Source, position: Option<&Position>) -> Problem {
    source.problem(
        source.locate_position(position),
        "This Expression contains a blank line, which usually means a stray quote or brace has swallowed the text after it.",
        Some("Check its quotes. For literal braces, write `\\{`.".into()),
    )
}

// ---------------------------------------------------------------------------
// Props
// ---------------------------------------------------------------------------

struct PropSpec {
    kind: Option<String>,
    default: Option<Value>,
    allowed: Option<Vec<Value>>,
}

struct Schema {
    props: BTreeMap<String, PropSpec>,
    required: Vec<String>,
}

fn read_schema(value: Option<&Value>) -> Schema {
    let mut schema = Schema { props: BTreeMap::new(), required: Vec::new() };
    let Some(Value::Object(object)) = value else {
        return schema;
    };
    if let Some(Value::Array(required)) = object.get("required") {
        schema.required = required.iter().map(|v| v.to_kstr().to_string()).collect();
    }
    if let Some(Value::Object(properties)) = object.get("properties") {
        for (name, spec) in properties.iter() {
            let Value::Object(spec) = spec else { continue };
            let allowed = match spec.get("enum") {
                Some(Value::Array(a)) => Some(a.clone()),
                _ => None,
            };
            schema.props.insert(name.to_string(), PropSpec { kind: spec.get("type").map(|t| t.to_kstr().to_string()), default: spec.get("default").cloned(), allowed });
        }
    }
    schema
}

fn type_name(value: &Value) -> String {
    match value {
        Value::Scalar(s) => s.type_name().to_string(),
        Value::Array(_) => "array".into(),
        Value::Object(_) => "object".into(),
        Value::Nil => "null".into(),
        Value::State(_) => "state".into(),
    }
}

fn type_matches(kind: &str, value: &Value) -> bool {
    let actual = type_name(value);
    match kind {
        "string" => actual == "string",
        "integer" => actual == "whole number",
        "number" => actual == "whole number" || actual == "fractional number",
        "boolean" => actual == "boolean",
        other => actual == other,
    }
}

// ---------------------------------------------------------------------------
// Stage 4: Expansion
// ---------------------------------------------------------------------------

struct Frame<'f> {
    source: &'f Source,
    scope: &'f liquid::Object,
    children: Option<&'f [Node]>,
}

struct Expander<'e> {
    files: &'e dyn Files,
    engines: &'e Engines,
    stack: Vec<String>,
    uses: usize,
    notes: Vec<Node>,
}

enum Evaluated {
    Comment,
    Value(Value),
}

fn is_phrasing(node: &Node) -> bool {
    matches!(node, Node::Break(_) | Node::Delete(_) | Node::Emphasis(_) | Node::FootnoteReference(_) | Node::Image(_) | Node::InlineCode(_) | Node::Link(_) | Node::Strong(_) | Node::Text(_))
}

/// Unravel: a paragraph of only tags and whitespace is block level.
fn unravels(paragraph: &Paragraph) -> bool {
    paragraph.children.iter().any(|c| matches!(c, Node::MdxJsxTextElement(_)))
        && paragraph.children.iter().all(|c| match c {
            Node::MdxJsxTextElement(_) => true,
            Node::Text(t) => t.value.trim().is_empty(),
            _ => false,
        })
}

fn text_node(value: String) -> Node {
    Node::Text(Text { value, position: None })
}

fn paragraph(children: Vec<Node>) -> Node {
    Node::Paragraph(Paragraph { children, position: None })
}

/// Phrasing children placed in a block position get a paragraph. Blocks stay as they are.
fn to_flow(nodes: &[Node]) -> Vec<Node> {
    let mut out = Vec::new();
    let mut run = Vec::new();
    for node in nodes {
        if is_phrasing(node) {
            run.push(node.clone());
        } else {
            flush_run(&mut run, &mut out);
            out.push(node.clone());
        }
    }
    flush_run(&mut run, &mut out);
    out
}

fn flush_run(run: &mut Vec<Node>, out: &mut Vec<Node>) {
    let mut nodes = std::mem::take(run);
    if let Some(Node::Text(first)) = nodes.first_mut() {
        first.value = first.value.trim_start_matches(EDGE).to_string();
    }
    if let Some(Node::Text(last)) = nodes.last_mut() {
        last.value.truncate(last.value.trim_end_matches(EDGE).len());
    }
    nodes.retain(|n| !matches!(n, Node::Text(t) if t.value.is_empty()));
    if !nodes.is_empty() {
        out.push(paragraph(nodes));
    }
}

impl Expander<'_> {
    fn evaluate(&self, raw: &str, frame: &Frame, in_cell: bool, position: Option<&Position>) -> Result<Evaluated, Problem> {
        let source = frame.source;
        let at = || source.locate_position(position);
        let unescaped = if in_cell { raw.replace("\\|", "|") } else { raw.to_string() };
        let trimmed = unescaped.trim();
        if trimmed.starts_with("/*") && trimmed.ends_with("*/") {
            return Ok(Evaluated::Comment);
        }
        if trimmed.is_empty() {
            return Err(source.problem(at(), "This Expression is empty.", Some("For literal braces, write `\\{}`.".into())));
        }
        let literal_hint = |trimmed: &str| {
            if trimmed.starts_with('{') {
                Some("`{% raw %}` only stops Liquid, and its output is still read as MDX. In prose, write `\\{\\{`.".to_string())
            } else if trimmed.starts_with('%') {
                Some("`{% raw %}` only stops Liquid, and its output is still read as MDX. In prose, write `\\{\\%`.".to_string())
            } else {
                None
            }
        };
        if has_delimiter_outside_string(trimmed) {
            return Err(source.problem(at(), format!("An Expression can't contain `{{{{`, `}}}}`, `{{%`, or `%}}`: `{{{trimmed}}}`."), literal_hint(trimmed)));
        }
        let prepared = tabs_to_spaces_outside_strings(trimmed);
        match capture(&self.engines.expression, &prepared, frame.scope) {
            Ok(value) => Ok(Evaluated::Value(value)),
            Err(Failure::Panic) => Err(Problem { file: source.name.clone(), at: at(), message: format!("Liquid crashed on the Expression `{{{trimmed}}}`."), hint: None, internal: true }),
            Err(Failure::Parse(message) | Failure::Render(message)) => {
                if let Some(name) = field(&message, "requested variable=") {
                    let body = &source.body;
                    let local = body.contains(&format!("for {name} in")) || body.contains(&format!("assign {name} ")) || body.contains(&format!("capture {name} "));
                    let hint = if local {
                        format!("`{name}` only exists during the Liquid Pass, and Expressions run after it. Use `{{{{ {name} }}}}`, or `{{{{ {name} | text }}}}` for data, and `{{{{ {name} | attr }}}}` in an attribute.")
                    } else {
                        "For literal braces, write `\\{`.".to_string()
                    };
                    return Err(source.problem(at(), format!("`{name}` is undefined."), Some(hint)));
                }
                if let (Some(name), Some(variable)) = (field(&message, "requested index="), field(&message, "variable=")) {
                    return Err(source.problem(at(), format!("`{name}` is undefined in `{variable}`."), None));
                }
                if let Some(name) = field(&message, "requested filter=") {
                    let hint = matches!(name, "text" | "attr").then(|| "An Expression's result is already data, so it needs neither filter.".to_string());
                    let message = if hint.is_some() { format!("`{name}` isn't available in an Expression, because its result is already data.") } else { format!("There's no `{name}` filter.") };
                    return Err(source.problem(at(), message, hint));
                }
                let reason = parse_error_reason(&message);
                let reason = reason.trim_end_matches('.');
                Err(source.problem(at(), format!("Couldn't read the Expression `{{{trimmed}}}`: {reason}."), literal_hint(trimmed)))
            }
        }
    }

    fn expression_text(&self, raw: &str, frame: &Frame, in_cell: bool, position: Option<&Position>) -> Result<Option<String>, Problem> {
        let Evaluated::Value(value) = self.evaluate(raw, frame, in_cell, position)? else {
            return Ok(None);
        };
        let printed = match &value {
            Value::Scalar(s) => s.to_kstr().to_string(),
            Value::Nil => String::new(),
            other => {
                let name = type_name(other);
                let article = if name == "array" { "an array" } else { "an object" };
                return Err(frame.source.problem(frame.source.locate_position(position), format!("`{{{}}}` is {article}, which can't print as text.", raw.trim()), Some("Pass it to a Component as a Prop, or loop over it with `{% for %}`.".into())));
            }
        };
        Ok(Some(printed.trim_matches(EDGE).to_string()))
    }

    fn check_name(&self, name: Option<&str>, frame: &Frame, position: Option<&Position>) -> Result<String, Problem> {
        let source = frame.source;
        let at = || source.locate_position(position);
        let Some(name) = name else {
            return Err(source.problem(at(), "Fragments such as `<>` aren't available.", Some("Remove them, because a Component's children need no wrapper.".into())));
        };
        if name.contains('.') || name.contains(':') {
            return Err(source.problem(at(), format!("`<{name}>`: namespaced Components aren't available yet."), None));
        }
        if name.chars().next().is_some_and(char::is_lowercase) {
            return Err(source.problem(at(), format!("`<{name}>`: lowercase tags such as `{name}` aren't available, because a document only produces Markdown."), Some("Use a Component, or write a literal `<` as `\\<`.".into())));
        }
        Ok(name.to_string())
    }

    fn attributes(&self, attributes: &[AttributeContent], frame: &Frame, in_cell: bool, position: Option<&Position>) -> Result<liquid::Object, Problem> {
        let source = frame.source;
        let mut props = liquid::Object::new();
        for attribute in attributes {
            let AttributeContent::Property(property) = attribute else {
                return Err(source.problem(source.locate_position(position), "Spread attributes such as `{...x}` aren't available.", Some("Pass each Prop by name.".into())));
            };
            let value = match &property.value {
                None => Value::scalar(true),
                // An author's `a\|b` in a table cell keeps its backslash in the value. Values
                // from `attr` spell the pipe `&#124;`, so a `\|` they decode to is real data, and
                // only the tag's own source can tell the two apart.
                Some(AttributeValue::Literal(value)) => {
                    let raw = position.map_or("", |p| &source.rendered[p.start.offset..p.end.offset]);
                    Value::scalar(if in_cell && raw.contains("\\|") { value.replace("\\|", "|") } else { value.clone() })
                }
                Some(AttributeValue::Expression(e)) => match self.evaluate(&e.value, frame, in_cell, position)? {
                    Evaluated::Value(value) => value,
                    Evaluated::Comment => return Err(source.problem(source.locate_position(position), format!("`{}` has a comment as its value.", property.name), None)),
                },
            };
            props.insert(property.name.clone().into(), value);
        }
        Ok(props)
    }

    fn content(&self, attributes: &[AttributeContent], children: &[Node], frame: &Frame, position: Option<&Position>) -> Result<Vec<Node>, Problem> {
        let source = frame.source;
        let at = || source.locate_position(position);
        if !attributes.is_empty() || !children.is_empty() {
            return Err(source.problem(at(), "`<Content />` takes no attributes or children.", None));
        }
        frame.children.map(<[Node]>::to_vec).ok_or_else(|| source.problem(at(), "`<Content />` has nothing to place in the Entry Document, because it isn't a Component.", None))
    }

    fn block_tag(&mut self, name: Option<&str>, attributes: &[AttributeContent], children: Vec<Node>, inline_children: bool, position: Option<&Position>, frame: &Frame, out: &mut Vec<Node>) -> Result<(), Problem> {
        let name = self.check_name(name, frame, position)?;
        if name == "Content" {
            out.extend(to_flow(&self.content(attributes, &children, frame, position)?));
            return Ok(());
        }
        let expanded = if inline_children { self.phrasing(children, frame, false)? } else { self.flow_vec(children, frame)? };
        let props = self.attributes(attributes, frame, false, position)?;
        out.extend(self.instantiate(&name, props, expanded, position, frame)?);
        Ok(())
    }

    fn inline_tag(&mut self, name: Option<&str>, attributes: &[AttributeContent], children: Vec<Node>, position: Option<&Position>, frame: &Frame, in_cell: bool, out: &mut Vec<Node>) -> Result<(), Problem> {
        let source = frame.source;
        let name = self.check_name(name, frame, position)?;
        let placed = if name == "Content" {
            self.content(attributes, &children, frame, position)?
        } else {
            let expanded = self.phrasing(children, frame, in_cell)?;
            let props = self.attributes(attributes, frame, in_cell, position)?;
            self.instantiate(&name, props, expanded, position, frame)?
        };
        if placed.iter().all(is_phrasing) {
            out.extend(placed);
            return Ok(());
        }
        let blocks = to_flow(&placed);
        match blocks.as_slice() {
            [] => {}
            [Node::Paragraph(p)] => out.extend(p.children.clone()),
            [one] => return Err(source.problem(source.locate_position(position), format!("`<{name}>` is used inside a line of text, but it expands to a {}, not a paragraph.", kind(one)), Some("Put it on a line of its own.".into()))),
            _ => return Err(source.problem(source.locate_position(position), format!("`<{name}>` is used inside a line of text or a table cell, but it expands to several blocks."), Some("Put it on a line of its own, or give it a single paragraph.".into()))),
        }
        Ok(())
    }

    fn flow_vec(&mut self, nodes: Vec<Node>, frame: &Frame) -> Result<Vec<Node>, Problem> {
        let mut out = Vec::with_capacity(nodes.len());
        self.flow(nodes, frame, &mut out)?;
        Ok(out)
    }

    /// The parser reads an Expression alone on a line as a block, even between lines of text,
    /// so a sentence wrapped around `{total}` would silently become three paragraphs. Lines
    /// with no blank line between them are one paragraph, as they are in Markdown.
    fn rejoin_lines(&mut self, nodes: Vec<Node>, frame: &Frame) -> Result<Vec<Node>, Problem> {
        let mut out: Vec<Node> = Vec::with_capacity(nodes.len());
        let mut last_end = None;
        for node in nodes {
            let start = node.position().map(|p| p.start.line);
            let end = node.position().map(|p| p.end.line);
            let touches = matches!((last_end, start), (Some(e), Some(s)) if s == e + 1);
            let joins = match (out.last(), &node) {
                (Some(Node::Paragraph(p)), Node::MdxFlowExpression(_)) => !unravels(p),
                (Some(Node::MdxFlowExpression(_)), Node::Paragraph(p)) => !unravels(p),
                (Some(Node::MdxFlowExpression(_)), Node::MdxFlowExpression(_)) => true,
                // Real paragraphs never touch, so the first of these has already been joined.
                (Some(Node::Paragraph(_)), Node::Paragraph(p)) => !unravels(p),
                _ => false,
            };
            if touches && joins {
                let previous = out.pop().expect("joins needs a previous node");
                let mut children = self.line_contents(previous, frame)?;
                let next = self.line_contents(node, frame)?;
                if !children.is_empty() && !next.is_empty() {
                    children.push(text_node("\n".to_string()));
                }
                children.extend(next);
                out.push(paragraph(children));
            } else {
                out.push(node);
            }
            last_end = end;
        }
        Ok(out)
    }

    fn line_contents(&mut self, node: Node, frame: &Frame) -> Result<Vec<Node>, Problem> {
        match node {
            Node::Paragraph(p) => Ok(p.children),
            Node::MdxFlowExpression(e) => Ok(self.expression_text(&e.value, frame, false, e.position.as_ref())?.filter(|t| !t.is_empty()).map(text_node).into_iter().collect()),
            other => Ok(vec![other]),
        }
    }

    fn flow(&mut self, nodes: Vec<Node>, frame: &Frame, out: &mut Vec<Node>) -> Result<(), Problem> {
        for node in self.rejoin_lines(nodes, frame)? {
            match node {
                Node::MdxFlowExpression(e) => {
                    if let Some(text) = self.expression_text(&e.value, frame, false, e.position.as_ref())? {
                        if !text.is_empty() {
                            out.push(paragraph(vec![text_node(text)]));
                        }
                    }
                }
                Node::MdxJsxFlowElement(e) => self.block_tag(e.name.as_deref(), &e.attributes, e.children, false, e.position.as_ref(), frame, out)?,
                Node::Paragraph(p) if unravels(&p) => {
                    for child in p.children {
                        if let Node::MdxJsxTextElement(e) = child {
                            self.block_tag(e.name.as_deref(), &e.attributes, e.children, true, e.position.as_ref(), frame, out)?;
                        }
                    }
                }
                Node::Paragraph(mut p) => {
                    p.children = self.phrasing(p.children, frame, false)?;
                    out.push(Node::Paragraph(p));
                }
                Node::Heading(mut h) => {
                    h.children = self.phrasing(h.children, frame, false)?;
                    out.push(Node::Heading(h));
                }
                Node::Blockquote(mut q) => {
                    q.children = self.flow_vec(q.children, frame)?;
                    out.push(Node::Blockquote(q));
                }
                Node::List(mut l) => {
                    l.children = self.flow_vec(l.children, frame)?;
                    out.push(Node::List(l));
                }
                Node::ListItem(mut i) => {
                    i.children = self.flow_vec(i.children, frame)?;
                    out.push(Node::ListItem(i));
                }
                Node::Table(mut t) => {
                    for row in &mut t.children {
                        let Node::TableRow(row) = row else { continue };
                        for cell in &mut row.children {
                            let Node::TableCell(cell) = cell else { continue };
                            cell.children = self.phrasing(std::mem::take(&mut cell.children), frame, true)?;
                        }
                    }
                    out.push(Node::Table(t));
                }
                other => out.push(other),
            }
        }
        Ok(())
    }

    fn phrasing(&mut self, nodes: Vec<Node>, frame: &Frame, in_cell: bool) -> Result<Vec<Node>, Problem> {
        let mut out = Vec::with_capacity(nodes.len());
        for node in nodes {
            match node {
                Node::MdxTextExpression(e) => {
                    if let Some(text) = self.expression_text(&e.value, frame, in_cell, e.position.as_ref())? {
                        out.push(text_node(text));
                    }
                }
                Node::MdxJsxTextElement(e) => self.inline_tag(e.name.as_deref(), &e.attributes, e.children, e.position.as_ref(), frame, in_cell, &mut out)?,
                Node::Emphasis(mut n) => {
                    n.children = self.phrasing(n.children, frame, in_cell)?;
                    out.push(Node::Emphasis(n));
                }
                Node::Strong(mut n) => {
                    n.children = self.phrasing(n.children, frame, in_cell)?;
                    out.push(Node::Strong(n));
                }
                Node::Delete(mut n) => {
                    n.children = self.phrasing(n.children, frame, in_cell)?;
                    out.push(Node::Delete(n));
                }
                Node::Link(mut n) => {
                    n.children = self.phrasing(n.children, frame, in_cell)?;
                    out.push(Node::Link(n));
                }
                other => out.push(other),
            }
        }
        Ok(out)
    }

    fn lift_notes(&mut self, notes: Vec<Node>, frame: &Frame) -> Result<(), Problem> {
        for note in notes {
            let Node::FootnoteDefinition(mut note) = note else { continue };
            note.children = self.flow_vec(note.children, frame)?;
            self.notes.push(Node::FootnoteDefinition(note));
        }
        Ok(())
    }

    fn read_source(&self, path: &str) -> Option<Result<(String, String, usize), Problem>> {
        let read = self.files.read(path)?;
        Some(read.map_err(|message| Problem { file: path.to_string(), at: Where::File, message, hint: None, internal: false }).and_then(|raw| split_frontmatter(path, &raw)))
    }

    fn instantiate(&mut self, name: &str, given: liquid::Object, children: Vec<Node>, position: Option<&Position>, caller: &Frame) -> Result<Vec<Node>, Problem> {
        let caller_at = || caller.source.locate_position(position);
        if let Some(first) = self.stack.iter().position(|s| s == name) {
            let chain = self.stack[first..].join(" -> ");
            return Err(caller.source.problem(caller_at(), format!("This is a recursion: {chain} -> {name}."), Some("A Component can't use itself, even under a Liquid condition.".into())));
        }
        let candidates = [format!("components/{name}.mdx"), format!("{name}.mdx")];
        let Some((path, read)) = candidates.iter().find_map(|path| self.read_source(path).map(|read| (path.clone(), read))) else {
            return Err(caller.source.problem(caller_at(), format!("`{name}` isn't a Component: there's no components/{name}.mdx or {name}.mdx."), None));
        };
        let (yaml, body, offset) = read?;
        let mut meta = parse_frontmatter(&path, &yaml)?;
        if meta.contains_key("meta") {
            return Err(Problem { file: path, at: Where::File, message: "The frontmatter key `meta` is reserved.".into(), hint: None, internal: false });
        }
        let schema = read_schema(meta.remove("props").as_ref());
        let props = self.validate(name, given, &schema, caller, position)?;
        let mut scope = meta.clone();
        scope.insert("meta".into(), Value::Object(meta));
        scope.insert("props".into(), Value::Object(props));

        let mut source = Source { name: path, body, offset, rendered: String::new() };
        source.rendered = liquid_pass(self.engines, &source, &scope)?;
        let mut tree = parse_dialect(&source)?;
        self.uses += 1;
        let notes = localize_references(&mut tree, Some(&format!("u{}", self.uses)));
        let Node::Root(root) = tree else {
            return Err(source.problem(Where::File, "The parser gave no root.", None));
        };
        self.stack.push(name.to_string());
        let frame = Frame { source: &source, scope: &scope, children: Some(&children) };
        let result = self.lift_notes(notes, &frame).and_then(|()| self.flow_vec(root.children, &frame));
        self.stack.pop();
        result
    }

    fn validate(&self, name: &str, given: liquid::Object, schema: &Schema, caller: &Frame, position: Option<&Position>) -> Result<liquid::Object, Problem> {
        let at = || caller.source.locate_position(position);
        for (prop, value) in given.iter() {
            let prop = prop.to_string();
            let Some(spec) = schema.props.get(&prop) else {
                return Err(caller.source.problem(at(), format!("`{prop}` isn't a declared Prop of `{name}`."), Some(format!("Declare it in the frontmatter of `{name}`, under `props`."))));
            };
            if let Some(kind) = &spec.kind {
                if !type_matches(kind, value) {
                    return Err(caller.source.problem(at(), format!("`{prop}` should be {kind}, and this is {}.", type_name(value)), Some(format!("A quoted attribute is a string. Pass other values as `{prop}={{3}}`."))));
                }
            }
            if let Some(allowed) = &spec.allowed {
                if !allowed.iter().any(|a| a.to_kstr() == value.to_kstr()) {
                    let names: Vec<String> = allowed.iter().map(|a| a.to_kstr().to_string()).collect();
                    return Err(caller.source.problem(at(), format!("`{prop}` must be one of {}, not {}.", names.join(", "), value.to_kstr()), None));
                }
            }
        }
        let mut props = given;
        for (prop, spec) in &schema.props {
            if !props.contains_key(prop.as_str()) {
                if let Some(default) = &spec.default {
                    props.insert(prop.clone().into(), default.clone());
                }
            }
        }
        for prop in &schema.required {
            if !props.contains_key(prop.as_str()) {
                return Err(caller.source.problem(at(), format!("`{prop}` is required by `{name}`."), None));
            }
        }
        Ok(props)
    }
}

fn has_mdx(node: &Node) -> bool {
    matches!(node, Node::MdxFlowExpression(_) | Node::MdxTextExpression(_) | Node::MdxJsxFlowElement(_) | Node::MdxJsxTextElement(_) | Node::MdxjsEsm(_) | Node::Html(_))
        || node.children().is_some_and(|c| c.iter().any(has_mdx))
}

// ---------------------------------------------------------------------------
// The whole of it
// ---------------------------------------------------------------------------

/// A Project to Intermediate Markdown. `data` replaces frontmatter keys, for generated tests.
pub fn expand(files: &dyn Files, engines: &Engines, data: Option<&liquid::Object>) -> Result<String, Problem> {
    let entry = "index.mdx";
    for reserved in ["components/Content.mdx", "Content.mdx"] {
        if files.read(reserved).is_some() {
            return Err(Problem { file: reserved.to_string(), at: Where::File, message: "The name `Content` is reserved for `<Content />`, so a Component can't use it.".into(), hint: None, internal: false });
        }
    }
    let mut expander = Expander { files, engines, stack: Vec::new(), uses: 0, notes: Vec::new() };
    let Some(read) = expander.read_source(entry) else {
        return Err(Problem { file: entry.to_string(), at: Where::File, message: "A Project needs an index.mdx.".into(), hint: None, internal: false });
    };
    let (yaml, body, offset) = read?;
    let mut meta = parse_frontmatter(entry, &yaml)?;
    for key in ["meta", "props"] {
        if meta.contains_key(key) {
            let message = if key == "meta" { "The frontmatter key `meta` is reserved.".to_string() } else { "The frontmatter key `props` declares a Component's Props, and the Entry Document isn't a Component.".to_string() };
            return Err(Problem { file: entry.to_string(), at: Where::File, message, hint: None, internal: false });
        }
    }
    if let Some(data) = data {
        for (key, value) in data.iter() {
            meta.insert(key.to_string().into(), value.clone());
        }
    }
    let mut scope = meta.clone();
    scope.insert("meta".into(), Value::Object(meta));
    scope.insert("props".into(), Value::Object(liquid::Object::new()));

    let mut source = Source { name: entry.to_string(), body, offset, rendered: String::new() };
    source.rendered = liquid_pass(engines, &source, &scope)?;
    let mut tree = parse_dialect(&source)?;
    let notes = localize_references(&mut tree, None);
    let Node::Root(root) = tree else {
        return Err(source.problem(Where::File, "The parser gave no root.", None));
    };
    let frame = Frame { source: &source, scope: &scope, children: None };
    expander.lift_notes(notes, &frame)?;
    let mut children = expander.flow_vec(root.children, &frame)?;

    // Children placed twice share their definition, so keep the first of each.
    let mut seen = HashSet::new();
    for note in std::mem::take(&mut expander.notes) {
        if let Node::FootnoteDefinition(n) = &note {
            if seen.insert(n.identifier.clone()) {
                children.push(note);
            }
        }
    }
    finish(Node::Root(Root { children, position: None }), &source)
}

/// Stage 5, and the round-trip check.
fn finish(mut tree: Node, source: &Source) -> Result<String, Problem> {
    normalize(&mut tree);
    let internal = |message: String| Problem { file: source.name.clone(), at: Where::File, message, hint: None, internal: true };
    if has_mdx(&tree) {
        return Err(internal("MDX survived Expansion.".into()));
    }
    let depth = depth_of(&tree);
    if depth > MAX_DEPTH {
        return Err(source.problem(Where::File, format!("This document is nested {depth} levels deep, and the limit is {MAX_DEPTH}."), None));
    }
    let events = to_events(&tree).map_err(|m| internal(format!("The tree isn't normalized: {m}")))?;
    let not_markdown = |detail: String| Problem {
        file: source.name.clone(),
        at: Where::File,
        message: format!("The expanded document can't be represented exactly in Markdown: {detail}."),
        hint: Some("`--emit-md` shows the Markdown. Common causes are a link inside a link, two hard breaks in a row, and a table cell ending in a backslash.".into()),
        internal: false,
    };
    let markdown = match catch_unwind(AssertUnwindSafe(|| serialize(&events))) {
        Err(_) => return Err(internal("The serializer crashed.".into())),
        Ok(Err(message)) => return Err(not_markdown(message)),
        Ok(Ok(markdown)) => markdown,
    };
    let reparsed = catch_unwind(AssertUnwindSafe(|| parse_markdown(&markdown))).map_err(|_| internal("pulldown-cmark crashed.".into()))?;
    let expected = canonical(events);
    if reparsed != expected {
        return Err(not_markdown(first_difference(&expected, &reparsed)));
    }
    Ok(markdown)
}
