//! Stage 2 and Expressions: everything `liquid` needs around it, from Spike 3 and the review
//! of Spikes 3 and 4.

use std::cell::RefCell;

use liquid::model::Value;
use liquid_core::model::DateTime;
use liquid_core::{
    Display_filter, Expression, Filter, FilterParameters, FilterReflection, FromFilterParameters,
    ParseFilter, Result as LiquidResult, Runtime, ValueView,
};

/// The characters a Markdown parser strips from the edges of a block.
pub const EDGE: [char; 4] = [' ', '\t', '\n', '\r'];

thread_local! {
    static CAPTURED: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, ParseFilter, FilterReflection)]
#[filter(name = "__capture", description = "Keeps the typed value.", parsed(CaptureFilter))]
struct Capture;

#[derive(Debug, Default, Display_filter)]
#[name = "__capture"]
struct CaptureFilter;

impl Filter for CaptureFilter {
    fn evaluate(&self, input: &dyn ValueView, _runtime: &dyn Runtime) -> LiquidResult<Value> {
        let value = input.to_value();
        CAPTURED.with(|c| c.borrow_mut().push(value));
        Ok(Value::Nil)
    }
}

pub fn attr_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('|', "&#124;")
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
}

#[derive(Clone, ParseFilter, FilterReflection)]
#[filter(name = "attr", description = "Writes a value safely in a quoted attribute.", parsed(AttrFilter))]
struct Attr;

#[derive(Debug, Default, Display_filter)]
#[name = "attr"]
struct AttrFilter;

impl Filter for AttrFilter {
    fn evaluate(&self, input: &dyn ValueView, _runtime: &dyn Runtime) -> LiquidResult<Value> {
        Ok(Value::scalar(attr_escape(&input.to_kstr())))
    }
}

pub fn text_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.trim_matches(EDGE).chars() {
        match c {
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' => out.push_str("&#9;"),
            c if c.is_ascii_punctuation() => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

#[derive(Clone, ParseFilter, FilterReflection)]
#[filter(name = "text", description = "Prints a value as literal text.", parsed(TextFilter))]
struct TextPrint;

#[derive(Debug, Default, Display_filter)]
#[name = "text"]
struct TextFilter;

impl Filter for TextFilter {
    fn evaluate(&self, input: &dyn ValueView, _runtime: &dyn Runtime) -> LiquidResult<Value> {
        Ok(Value::scalar(text_escape(&input.to_kstr())))
    }
}

#[derive(Debug, FilterParameters)]
struct FixedDateArgs {
    #[parameter(description = "The format to return the date in.", arg_type = "str")]
    format: Expression,
}

#[derive(Clone, ParseFilter, FilterReflection)]
#[filter(name = "date", description = "The stock date filter, refusing the clock.", parameters(FixedDateArgs), parsed(FixedDateFilter))]
struct FixedDate;

#[derive(Debug, FromFilterParameters, Display_filter)]
#[name = "date"]
struct FixedDateFilter {
    #[parameters]
    args: FixedDateArgs,
}

impl Filter for FixedDateFilter {
    fn evaluate(&self, input: &dyn ValueView, runtime: &dyn Runtime) -> LiquidResult<Value> {
        let args = self.args.evaluate(runtime)?;
        let word = input.to_kstr().trim().to_ascii_lowercase();
        if word == "now" || word == "today" {
            return Err(liquid_core::Error::with_msg("`now` and `today` aren't available, because output must not depend on the clock. Put the date in frontmatter."));
        }
        match input.as_scalar().and_then(|s| parse_date(&s)) {
            Some(date) if !args.format.is_empty() => {
                let s = date
                    .format(args.format.as_str())
                    .map_err(|_| liquid_core::Error::with_msg(format!("Invalid date-format string: {}", args.format)))?;
                Ok(Value::scalar(s))
            }
            _ => Ok(input.to_value()),
        }
    }
}

/// The stock filter takes neither `2026-01-02` nor `2026-01-02T03:04:05Z`, the two ways YAML
/// writes a date, and silently ignores the format. So try those too.
fn parse_date(scalar: &liquid_core::model::ScalarCow<'_>) -> Option<DateTime> {
    if let Some(date) = scalar.to_date_time() {
        return Some(date);
    }
    let text = scalar.to_kstr();
    if let Some(date) = scalar.to_date() {
        return DateTime::from_str(&format!("{date} 00:00:00 +0000"));
    }
    DateTime::from_str(&text.replacen('T', " ", 1).replace('Z', " +0000"))
}

/// Two parsers: authors never see `__capture`, and Expressions don't get `text` and `attr`,
/// whose escaping would print, because an Expression's result is already data.
pub struct Engines {
    pub author: liquid::Parser,
    pub expression: liquid::Parser,
}

impl Engines {
    pub fn new() -> Self {
        Self {
            author: liquid::ParserBuilder::with_stdlib().filter(FixedDate).filter(Attr).filter(TextPrint).build().unwrap(),
            expression: liquid::ParserBuilder::with_stdlib().filter(FixedDate).filter(Capture).build().unwrap(),
        }
    }
}

pub enum Failure {
    Parse(String),
    Render(String),
    Panic,
}

pub fn render(parser: &liquid::Parser, text: &str, scope: &liquid::Object) -> Result<String, Failure> {
    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match parser.parse(text) {
        Err(e) => Err(Failure::Parse(e.to_string())),
        Ok(template) => template.render(scope).map_err(|e| Failure::Render(e.to_string())),
    }));
    attempt.unwrap_or(Err(Failure::Panic))
}

/// `{{ EXPR | __capture }}`. Output must be empty, with exactly one capture.
pub fn capture(parser: &liquid::Parser, expr: &str, scope: &liquid::Object) -> Result<Value, Failure> {
    CAPTURED.with(|c| c.borrow_mut().clear());
    let out = render(parser, &format!("{{{{ {expr} | __capture }}}}"), scope)?;
    let mut captured = CAPTURED.with(|c| std::mem::take(&mut *c.borrow_mut()));
    if !out.is_empty() || captured.len() != 1 {
        return Err(Failure::Parse(format!("  = printed {out:?}, with {} captures", captured.len())));
    }
    Ok(captured.remove(0))
}

/// A check before wrapping: no Liquid delimiter outside a string.
pub fn has_delimiter_outside_string(expr: &str) -> bool {
    let bytes = expr.as_bytes();
    let mut quote = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'"' || c == b'\'' => quote = Some(c),
            None if i + 1 < bytes.len() && matches!(&bytes[i..i + 2], b"}}" | b"{{" | b"%}" | b"{%") => return true,
            None => {}
        }
        i += 1;
    }
    false
}

/// Liquid's grammar only takes spaces and line endings as whitespace.
pub fn tabs_to_spaces_outside_strings(expr: &str) -> String {
    let mut quote = None;
    expr.chars()
        .map(|c| match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                c
            }
            (Some(_), c) => c,
            (None, '"' | '\'') => {
                quote = Some(c);
                c
            }
            (None, '\t') => ' ',
            (None, c) => c,
        })
        .collect()
}
