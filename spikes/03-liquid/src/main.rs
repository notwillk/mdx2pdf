//! Spike 3: does `liquid` do what both the Liquid Pass and Expressions need?
//!
//! Usage:
//!   spike-liquid run <label>      Deterministic suites, into results/<label>/
//!   spike-liquid deep <label>     Depth and loops in child processes, plus process order
//!   spike-liquid one <template>   Prints the outcome with the table suite's data
//!   spike-liquid child ...        Internal

mod cases;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cases::Want;
use liquid::model::Value;
use liquid_core::{Display_filter, Expression, Filter, FilterParameters, FilterReflection, FromFilterParameters, ParseFilter, Result as LiquidResult, Runtime, ValueView};
use markdown::mdast::{AttributeContent, AttributeValue, Node};
use markdown::message::Place;
use markdown::{to_mdast, Constructs, ParseOptions};

thread_local! {
    static PANIC_LOCATION: RefCell<String> = const { RefCell::new(String::new()) };
    static CAPTURED: RefCell<Vec<Value>> = const { RefCell::new(Vec::new()) };
}

// ---------------------------------------------------------------------------
// Custom filters
// ---------------------------------------------------------------------------

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

fn attr_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;").replace('\r', "&#13;").replace('\n', "&#10;")
}

#[derive(Clone, ParseFilter, FilterReflection)]
#[filter(name = "attr", description = "Quotes a value safely as a tag attribute.", parsed(AttrFilter))]
struct Attr;

#[derive(Debug, Default, Display_filter)]
#[name = "attr"]
struct AttrFilter;

impl Filter for AttrFilter {
    fn evaluate(&self, input: &dyn ValueView, _runtime: &dyn Runtime) -> LiquidResult<Value> {
        Ok(Value::scalar(attr_escape(&input.to_kstr())))
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
        match input.as_scalar().and_then(|s| s.to_date_time()) {
            Some(date) if !args.format.is_empty() => {
                let s = date.format(args.format.as_str()).map_err(|_| liquid_core::Error::with_msg(format!("Invalid date-format string: {}", args.format)))?;
                Ok(Value::scalar(s))
            }
            _ => Ok(input.to_value()),
        }
    }
}

fn parser() -> liquid::Parser {
    liquid::ParserBuilder::with_stdlib().filter(Capture).filter(Attr).build().unwrap()
}

// ---------------------------------------------------------------------------
// Rendering and evaluation
// ---------------------------------------------------------------------------

fn one_line(text: &str) -> String {
    text.trim_end().replace('\n', " ⏎ ")
}

fn frontmatter(yaml: &str) -> Result<liquid::Object, String> {
    serde_saphyr::from_str::<liquid::Object>(yaml).map_err(|e| one_line(&e.to_string()))
}

fn scope(yaml: &str) -> liquid::Object {
    let front = frontmatter(yaml).expect("the harness's own frontmatter parses");
    let mut object = front.clone();
    object.insert("meta".into(), Value::Object(front));
    object
}

enum Rendered {
    Output(String),
    Parse(String),
    Render(String),
    Panic(String),
}

fn render(parser: &liquid::Parser, template: &str, globals: &liquid::Object) -> Rendered {
    let result = catch_unwind(AssertUnwindSafe(|| match parser.parse(template) {
        Err(e) => Rendered::Parse(e.to_string()),
        Ok(t) => match t.render(globals) {
            Ok(out) => Rendered::Output(out),
            Err(e) => Rendered::Render(e.to_string()),
        },
    }));
    result.unwrap_or_else(|_| Rendered::Panic(PANIC_LOCATION.with(|l| l.borrow().clone())))
}

fn describe(rendered: &Rendered) -> String {
    match rendered {
        Rendered::Output(out) => format!("{out:?}"),
        Rendered::Parse(e) => format!("PARSE ERROR: {}", one_line(e)),
        Rendered::Render(e) => format!("RENDER ERROR: {}", one_line(e)),
        Rendered::Panic(at) => format!("PANIC at {at}"),
    }
}

fn show(value: &Value) -> String {
    match value {
        Value::Scalar(s) => format!("{}:{}", s.type_name(), s.to_kstr()),
        Value::Array(a) => format!("[{}]", a.iter().map(show).collect::<Vec<_>>().join(",")),
        Value::Object(o) => {
            let sorted: BTreeMap<_, _> = o.iter().collect();
            format!("{{{}}}", sorted.iter().map(|(k, v)| format!("{k}:{}", show(v))).collect::<Vec<_>>().join(","))
        }
        Value::State(s) => format!("state:{s:?}"),
        Value::Nil => "nil".into(),
    }
}

/// Mechanism A: `{{ EXPR | __capture }}`. Output must be empty, with exactly one capture.
fn eval_output(parser: &liquid::Parser, expr: &str, globals: &liquid::Object) -> Result<Value, String> {
    eval_wrapped(parser, &format!("{{{{ {expr} | __capture }}}}"), globals)
}

/// Mechanism B: `{% assign %}`, then capture.
fn eval_assign(parser: &liquid::Parser, expr: &str, globals: &liquid::Object) -> Result<Value, String> {
    eval_wrapped(parser, &format!("{{% assign __v = {expr} %}}{{{{ __v | __capture }}}}"), globals)
}

fn eval_wrapped(parser: &liquid::Parser, template: &str, globals: &liquid::Object) -> Result<Value, String> {
    CAPTURED.with(|c| c.borrow_mut().clear());
    match render(parser, template, globals) {
        Rendered::Output(out) if !out.is_empty() => Err(format!("rejected: printed {out:?}")),
        Rendered::Output(_) => {
            let mut captured = CAPTURED.with(|c| std::mem::take(&mut *c.borrow_mut()));
            if captured.len() == 1 {
                Ok(captured.remove(0))
            } else {
                Err(format!("rejected: {} captures", captured.len()))
            }
        }
        other => Err(describe(&other)),
    }
}

/// A check before wrapping: no Liquid delimiter outside a string.
fn has_delimiter_outside_string(expr: &str) -> bool {
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

// ---------------------------------------------------------------------------
// Dialect parsing, copied from Spike 2
// ---------------------------------------------------------------------------

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
        Node::Strong(n) => group("Strong", &n.children),
        Node::Text(n) => format!("T({:?})", n.value),
        Node::Code(n) => format!("Code({:?})", n.value),
        Node::MdxFlowExpression(n) => format!("ExprF({:?})", n.value),
        Node::MdxTextExpression(n) => format!("ExprT({:?})", n.value),
        Node::MdxJsxFlowElement(n) => group(&format!("JsxF({}{})", n.name.clone().unwrap_or_default(), attributes(&n.attributes)), &n.children),
        Node::MdxJsxTextElement(n) => group(&format!("JsxT({}{})", n.name.clone().unwrap_or_default(), attributes(&n.attributes)), &n.children),
        other => format!("{other:?}").split([' ', '(']).next().unwrap_or("?").to_string(),
    }
}

enum Parsed {
    Tree(Node),
    Error { reason: String, line: Option<usize>, column: Option<usize> },
    Panic(String),
}

fn parse_dialect(text: &str) -> Parsed {
    match catch_unwind(AssertUnwindSafe(|| to_mdast(text, &dialect()))) {
        Ok(Ok(tree)) => Parsed::Tree(tree),
        Ok(Err(m)) => {
            let (line, column) = match m.place.as_deref() {
                Some(Place::Point(p)) => (Some(p.line), Some(p.column)),
                Some(Place::Position(p)) => (Some(p.start.line), Some(p.start.column)),
                None => (None, None),
            };
            Parsed::Error { reason: m.reason, line, column }
        }
        Err(_) => Parsed::Panic(PANIC_LOCATION.with(|l| l.borrow().clone())),
    }
}

fn describe_parsed(parsed: &Parsed) -> String {
    match parsed {
        Parsed::Tree(t) => compact(t),
        Parsed::Error { reason, line, column } => format!("DIALECT ERROR at {line:?}:{column:?}: {reason}"),
        Parsed::Panic(at) => format!("DIALECT PANIC at {at}"),
    }
}

fn count_quotes(node: &Node) -> usize {
    let own = usize::from(matches!(node, Node::Blockquote(_)));
    own + node.children().map_or(0, |c| c.iter().map(count_quotes).sum())
}

fn collect_expressions(node: &Node, out: &mut Vec<String>) {
    match node {
        Node::MdxFlowExpression(n) => out.push(n.value.clone()),
        Node::MdxTextExpression(n) => out.push(n.value.clone()),
        _ => {}
    }
    if let Some(children) = node.children() {
        children.iter().for_each(|c| collect_expressions(c, out));
    }
}

fn find_attribute(node: &Node) -> Option<String> {
    let found = match node {
        Node::MdxJsxFlowElement(n) => n.attributes.first(),
        Node::MdxJsxTextElement(n) => n.attributes.first(),
        _ => None,
    };
    if let Some(AttributeContent::Property(p)) = found {
        if let Some(AttributeValue::Literal(l)) = &p.value {
            return Some(l.clone());
        }
    }
    node.children()?.iter().find_map(find_attribute)
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

/// Removes characters while the same line keeps panicking.
fn reduce(parser: &liquid::Parser, input: &str, location: &str, globals: &liquid::Object) -> String {
    let same = |text: &str| matches!(render(parser, text, globals), Rendered::Panic(at) if at == location);
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
// Child processes
// ---------------------------------------------------------------------------

fn run_child(args: &[&str], timeout: Duration) -> String {
    let exe = std::env::current_exe().unwrap();
    let mut process = Command::new(exe).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let start = Instant::now();
    loop {
        match process.try_wait().unwrap() {
            Some(status) => {
                let output = process.wait_with_output().unwrap();
                let text = one_line(&String::from_utf8_lossy(&output.stdout));
                return if status.success() { text } else { format!("ABORT ({status}) after: {text}") };
            }
            None if start.elapsed() > timeout => {
                let _ = process.kill();
                let output = process.wait_with_output().unwrap();
                let text = one_line(&String::from_utf8_lossy(&output.stdout));
                return format!("timeout (over {}s) after: {text}", timeout.as_secs());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

fn deep_template(shape: &str, depth: usize) -> String {
    match shape {
        "if" => format!("{}x{}", "{% if true %}".repeat(depth), "{% endif %}".repeat(depth)),
        "for" => format!("{}x{}", "{% for i in (1..1) %}".repeat(depth), "{% endfor %}".repeat(depth)),
        "unless" => format!("{}x{}", "{% unless false %}".repeat(depth), "{% endunless %}".repeat(depth)),
        "capture" => format!("{}x{}", "{% capture a %}".repeat(depth), "{% endcapture %}".repeat(depth)),
        "path" => format!("{{{{ a{} }}}}", ".a".repeat(depth)),
        "index" => format!("{{{{ a{}{} }}}}", "[a".repeat(depth), "]".repeat(depth)),
        "filters" => format!("{{{{ x{} }}}}", " | upcase".repeat(depth)),
        other => panic!("unknown shape {other}"),
    }
}

fn child(args: &[String]) {
    match args[0].as_str() {
        "abort" => std::process::abort(),
        "order" => {
            let globals = scope("obj: {zebra: 1, apple: 2, mango: 3, kiwi: 4, fig: 5, lime: 6}\n");
            match render(&parser(), "{% for kv in obj %}{{ kv[0] }},{% endfor %}|{{ obj | json }}", &globals) {
                Rendered::Output(out) => println!("{out}"),
                other => {
                    let plain = render(&parser(), "{% for kv in obj %}{{ kv[0] }},{% endfor %}", &globals);
                    println!("{} (with json: {})", describe(&plain), describe(&other));
                }
            }
        }
        "deep" => {
            let template = deep_template(&args[1], args[2].parse().unwrap());
            let stack: usize = args[3].parse().unwrap();
            let work = move || {
                let parser = parser();
                let mut a = liquid::Object::new();
                a.insert("x".into(), Value::scalar("v"));
                let start = Instant::now();
                let parsed = catch_unwind(AssertUnwindSafe(|| parser.parse(&template)));
                match parsed {
                    Err(_) => println!("caught panic in parse at {}", PANIC_LOCATION.with(|l| l.borrow().clone())),
                    Ok(Err(e)) => println!("parse error: {}", one_line(&e.to_string()).chars().take(80).collect::<String>()),
                    Ok(Ok(t)) => {
                        println!("parsed {:.3}s", start.elapsed().as_secs_f64());
                        let start = Instant::now();
                        match catch_unwind(AssertUnwindSafe(|| t.render(&a))) {
                            Err(_) => println!("caught panic in render at {}", PANIC_LOCATION.with(|l| l.borrow().clone())),
                            Ok(Ok(out)) => println!("rendered {} bytes {:.3}s", out.len(), start.elapsed().as_secs_f64()),
                            Ok(Err(e)) => println!("render error: {}", one_line(&e.to_string()).chars().take(60).collect::<String>()),
                        }
                        drop(t);
                        println!("dropped");
                    }
                }
            };
            if stack == 0 {
                work();
            } else {
                std::thread::Builder::new().stack_size(stack).spawn(work).unwrap().join().unwrap();
            }
        }
        "loop" => {
            let start = Instant::now();
            let template = format!("{{% for i in (1..{}) %}}x{{% endfor %}}", args[1]);
            match render(&parser(), &template, &liquid::Object::new()) {
                Rendered::Output(out) => println!("{} bytes {:.3}s", out.len(), start.elapsed().as_secs_f64()),
                other => println!("{}", describe(&other).chars().take(120).collect::<String>()),
            }
        }
        other => panic!("unknown child {other}"),
    }
}

fn run_deep(label: &str) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = format!("label: {label}\nTimeout 30s per case. Child output shows how far it got.\n\n");
    for shape in ["if", "for", "unless", "capture", "path", "index", "filters"] {
        for depth in [100usize, 1_000, 2_000, 5_000, 10_000, 100_000] {
            let d = depth.to_string();
            let result = run_child(&["child", "deep", shape, &d, "0"], Duration::from_secs(30));
            let mut line = format!("{shape:>8} depth {depth:>6}: {result}");
            let failed = result.starts_with("ABORT");
            if failed {
                let big = run_child(&["child", "deep", shape, &d, "1073741824"], Duration::from_secs(60));
                let _ = write!(line, "   | 1 GiB stack: {big}");
            }
            println!("{line}");
            let _ = writeln!(out, "{line}");
            if result.starts_with("timeout") {
                break;
            }
        }
    }
    let _ = writeln!(out, "\nlarge loops:");
    for n in ["1000000", "10000000", "100000000"] {
        let result = run_child(&["child", "loop", n], Duration::from_secs(30));
        let line = format!("(1..{n}): {result}");
        println!("{line}");
        let _ = writeln!(out, "{line}");
    }
    let _ = writeln!(out, "\nobject order across processes, YAML order is zebra,apple,mango,kiwi,fig,lime:");
    for _ in 0..6 {
        let result = run_child(&["child", "order"], Duration::from_secs(10));
        println!("{result}");
        let _ = writeln!(out, "{result}");
    }
    let dir = manifest.join("results").join(label);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("deep.txt"), out).unwrap();
}

// ---------------------------------------------------------------------------
// Controls and suites
// ---------------------------------------------------------------------------

fn controls() {
    let parser = parser();
    assert!(catch_unwind(|| panic!("deliberate")).is_err(), "control: the guard must catch a panic");
    let globals = scope("a: 3\nb: [x]\n");
    assert_eq!(show(&eval_output(&parser, "a", &globals).unwrap()), "whole number:3", "control: capture must see a number");
    assert_eq!(show(&eval_output(&parser, "b", &globals).unwrap()), "[string:x]", "control: capture must see an array");
    assert!(eval_output(&parser, "a }} leak {{ a", &globals).is_err(), "control: injected output must be detected");
    assert!(matches!(render(&parser, "{{ a }}", &globals), Rendered::Output(ref o) if o == "3"), "control: render must work");
    assert!(matches!(parse_dialect("<A />\n"), Parsed::Tree(_)), "control: the Dialect must parse");
    assert_eq!(attr_escape("a\"b&c"), "a&quot;b&amp;c", "control: attr must escape");
    let outcome = run_child(&["child", "abort"], Duration::from_secs(10));
    assert!(outcome.starts_with("ABORT"), "control: an aborting child must be reported, got {outcome}");
}

fn run_suites(label: &str) {
    controls();
    let parser = parser();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut summary = format!("label: {label}\n\n");
    let mut detail = String::new();

    // Table suite.
    let globals = scope(cases::data());
    let mut groups: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut surprises = Vec::new();
    for (group, template, want) in cases::CASES {
        let rendered = render(&parser, template, &globals);
        let actual = describe(&rendered);
        let matched = match (want, &rendered) {
            (Want::Out(o), Rendered::Output(out)) => o == out,
            (Want::Err, Rendered::Parse(_) | Rendered::Render(_)) => true,
            (Want::Named(n), Rendered::Parse(e) | Rendered::Render(e)) => e.contains(n),
            _ => false,
        };
        let entry = groups.entry(group).or_default();
        entry.0 += 1;
        if matched {
            entry.1 += 1;
        } else {
            let wanted = match want {
                Want::Out(o) => format!("{o:?}"),
                Want::Err => "an error".into(),
                Want::Named(n) => format!("an error naming {n}"),
            };
            surprises.push(format!("  SURPRISE [{group}] {template:?}\n    expected {wanted}\n    actual   {actual}"));
        }
        let _ = writeln!(detail, "[{group}] {template:?}\n  {actual}");
    }
    let _ = writeln!(summary, "table: {} cases, {} surprises", cases::CASES.len(), surprises.len());
    for (group, (total, matched)) in &groups {
        let _ = writeln!(summary, "  {group}: {matched}/{total}");
    }
    for surprise in &surprises {
        let _ = writeln!(summary, "{surprise}");
    }
    let _ = writeln!(summary, "\nundefined values, what each one does:");
    for (group, template, _) in cases::CASES {
        if *group == "undefined" {
            let _ = writeln!(summary, "  {template:?}\n    {}", describe(&render(&parser, template, &globals)));
        }
    }

    // Typed values, both mechanisms.
    let _ = writeln!(summary, "\ntyped values:");
    let mut scores = [0usize; 2];
    for (expr, expected) in cases::TYPED {
        let a = eval_output(&parser, expr, &globals).map_or_else(|e| format!("ERR {e}"), |v| show(&v));
        let b = eval_assign(&parser, expr, &globals).map_or_else(|e| format!("ERR {e}"), |v| show(&v));
        scores[0] += usize::from(a == *expected);
        scores[1] += usize::from(b == *expected);
        if a != *expected || b != *expected {
            let _ = writeln!(summary, "  {expr:?}\n    expected {expected}\n    output   {a}\n    assign   {b}");
        }
    }
    let _ = writeln!(summary, "  of {}: output capture {}, assign capture {}", cases::TYPED.len(), scores[0], scores[1]);

    // Injection.
    let _ = writeln!(summary, "\ntext that tries to escape its wrapper:");
    for expr in cases::INJECTION {
        let a = eval_output(&parser, expr, &globals).map_or_else(|e| format!("rejected ({})", e.chars().take(70).collect::<String>()), |v| format!("ACCEPTED {}", show(&v)));
        let b = eval_assign(&parser, expr, &globals).map_or_else(|e| format!("rejected ({})", e.chars().take(70).collect::<String>()), |v| format!("ACCEPTED {}", show(&v)));
        let _ = writeln!(summary, "  {expr:?}  pre-check flags it: {}\n    output {a}\n    assign {b}", has_delimiter_outside_string(expr));
    }

    let flagged_typed = cases::TYPED.iter().filter(|(expr, _)| has_delimiter_outside_string(expr)).count();
    let combined = |eval: &dyn Fn(&str) -> bool| cases::INJECTION.iter().filter(|expr| !has_delimiter_outside_string(expr) && eval(expr)).count();
    let _ = writeln!(summary, "  of {} attempts, accepted: output {}, assign {}, pre-check plus output {}, pre-check plus assign {}", cases::INJECTION.len(), cases::INJECTION.iter().filter(|e| eval_output(&parser, e, &globals).is_ok()).count(), cases::INJECTION.iter().filter(|e| eval_assign(&parser, e, &globals).is_ok()).count(), combined(&|e| eval_output(&parser, e, &globals).is_ok()), combined(&|e| eval_assign(&parser, e, &globals).is_ok()));
    let _ = writeln!(summary, "  valid typed Expressions the pre-check wrongly flags: {flagged_typed} of {}", cases::TYPED.len());

    // Attribute filter.
    let _ = writeln!(summary, "\nattr filter round trip through the Dialect, 20,000 random values:");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut totals: BTreeMap<&str, usize> = BTreeMap::new();
    let mut first: BTreeMap<String, String> = BTreeMap::new();
    for _ in 0..20_000 {
        let count = 1 + rng.below(6);
        let value: String = (0..count).map(|_| rng.pick(cases::VALUE_TOKENS)).collect();
        let mut g = liquid::Object::new();
        g.insert("t".into(), Value::scalar(value.clone()));
        for (name, template) in [("attr flow", "<Card title=\"{{ t | attr }}\" />\n"), ("attr text", "a <Card title=\"{{ t | attr }}\" /> b\n"), ("attr in quote", "> <Card title=\"{{ t | attr }}\" />\n"), ("no filter", "<Card title=\"{{ t }}\" />\n")] {
            let outcome = match render(&parser, template, &g) {
                Rendered::Output(out) => match parse_dialect(&out) {
                    Parsed::Tree(tree) => match find_attribute(&tree) {
                        Some(found) if found == value => "same",
                        Some(_) => "different value",
                        None => "no attribute",
                    },
                    Parsed::Error { .. } => "dialect error",
                    Parsed::Panic(_) => "dialect panic",
                },
                _ => "render failed",
            };
            *totals.entry(Box::leak(format!("{name}: {outcome}").into_boxed_str())).or_default() += 1;
            if outcome != "same" {
                first.entry(format!("{name}: {outcome}")).or_insert_with(|| format!("{value:?}"));
            }
        }
    }
    for (key, count) in &totals {
        let example = first.get(*key).map_or(String::new(), |v| format!("  first: {v}"));
        let _ = writeln!(summary, "  {count:>6} {key}{example}");
    }

    // YAML.
    let _ = writeln!(summary, "\nyaml through serde-saphyr to liquid values:");
    for yaml in cases::YAML {
        match frontmatter(yaml) {
            Ok(object) => {
                let mut printed = Vec::new();
                let sorted: BTreeMap<_, _> = object.iter().collect();
                for (key, value) in &sorted {
                    let mut g = liquid::Object::new();
                    g.insert("v".into(), (*value).clone());
                    printed.push(format!("{key} = {} prints {}", show(value), describe(&render(&parser, "{{ v }}", &g))));
                }
                let _ = writeln!(summary, "  {yaml:?}\n    {}", printed.join("; "));
            }
            Err(e) => {
                let _ = writeln!(summary, "  {yaml:?}\n    ERROR {}", e.chars().take(150).collect::<String>());
            }
        }
    }

    // Quote patterns, judged by the Dialect parser.
    let _ = writeln!(summary, "\nblockquote spellings, c true and false:");
    for (name, template) in cases::QUOTE_PATTERNS {
        let mut line = format!("  {name}:");
        for c in [true, false] {
            let mut g = liquid::Object::new();
            g.insert("c".into(), Value::scalar(c));
            if let Rendered::Output(out) = render(&parser, template, &g) {
                let result = match parse_dialect(&out) {
                    Parsed::Tree(t) => format!("{} quote(s) {}", count_quotes(&t), compact(&t)),
                    other => describe_parsed(&other),
                };
                let _ = write!(line, "\n    c={c}: {out:?} -> {result}");
            }
        }
        let _ = writeln!(summary, "{line}");
    }
    let looped = "> Items:\n{%- for i in xs %}\n> - {{ i }}\n{%- endfor %}\n";
    if let Rendered::Output(out) = render(&parser, looped, &scope("xs: [a, b]\n")) {
        let _ = writeln!(summary, "  loop in a quote: {out:?} -> {}", describe_parsed(&parse_dialect(&out)));
    }

    // End to end.
    let _ = writeln!(summary, "\nprd example:");
    let prd = render(&parser, cases::PRD_INDEX, &scope(cases::PRD_YAML));
    if let Rendered::Output(out) = &prd {
        let parsed = describe_parsed(&parse_dialect(out));
        let _ = writeln!(summary, "  index matches the expected tree: {}\n  {parsed}", parsed == cases::PRD_EXPECTED);
        if let Parsed::Tree(tree) = parse_dialect(out) {
            let mut exprs = Vec::new();
            collect_expressions(&tree, &mut exprs);
            let g = scope(cases::PRD_YAML);
            let values: Vec<_> = exprs.iter().map(|e| eval_output(&parser, e, &g).map_or_else(|e| e, |v| show(&v))).collect();
            let _ = writeln!(summary, "  its Expressions evaluate to: {values:?}");
        }
    }
    let mut props = liquid::Object::new();
    props.insert("kind".into(), Value::scalar("Alpha"));
    let mut component = liquid::Object::new();
    component.insert("props".into(), Value::Object(props));
    if let Rendered::Output(out) = render(&parser, cases::COMPONENT_BODY, &component) {
        let parsed = describe_parsed(&parse_dialect(&out));
        let _ = writeln!(summary, "  component matches the expected tree: {}\n  {parsed}", parsed == cases::COMPONENT_EXPECTED);
    }
    let _ = writeln!(summary, "  props.kind | upcase evaluates to: {:?}", eval_output(&parser, "props.kind | upcase", &component).map(|v| show(&v)));

    // Hazards.
    let _ = writeln!(summary, "\nknown hazards, exact messages:");
    for (name, yaml, body) in cases::HAZARDS {
        let rendered = render(&parser, body, &scope(yaml));
        let _ = writeln!(summary, "  {name}: {body:?}\n    liquid: {}", describe(&rendered));
        if let Rendered::Output(out) = rendered {
            let parsed = parse_dialect(&out);
            let _ = writeln!(summary, "    dialect: {}", describe_parsed(&parsed));
            if let Parsed::Tree(tree) = parsed {
                let mut exprs = Vec::new();
                collect_expressions(&tree, &mut exprs);
                fn attr_exprs(node: &Node, out: &mut Vec<String>) {
                    let list = match node {
                        Node::MdxJsxFlowElement(n) => Some(&n.attributes),
                        Node::MdxJsxTextElement(n) => Some(&n.attributes),
                        _ => None,
                    };
                    for attribute in list.into_iter().flatten() {
                        if let AttributeContent::Property(p) = attribute {
                            if let Some(AttributeValue::Expression(e)) = &p.value {
                                out.push(e.value.clone());
                            }
                        }
                    }
                    if let Some(children) = node.children() {
                        children.iter().for_each(|c| attr_exprs(c, out));
                    }
                }
                attr_exprs(&tree, &mut exprs);
                for expr in exprs {
                    let result = eval_output(&parser, &expr, &scope(yaml)).map_or_else(|e| e, |v| show(&v));
                    let _ = writeln!(summary, "    expression {expr:?}: {result}");
                }
            }
        }
    }

    // Error locations.
    let _ = writeln!(summary, "\nerror locations. Each body starts after 3 lines of frontmatter, so file line = body line + 3 or 4:");
    let located: &[(&str, &str, &str)] = &[
        ("parse error, body line 3", "x: 1\n", "one\ntwo\n{% if %}\nfour\n"),
        ("parse error, unclosed block from body line 2", "x: 1\n", "one\n{% if x %}\nthree\n"),
        ("parse error, unknown filter on body line 4", "x: 1\n", "one\n\n\n{{ x | nosuch }}\n"),
        ("render error, body line 4", "x: 1\n", "one\n\n\n{{ missing }}\n"),
        ("render error, in a loop and an if, on the third item", "xs: [{a: 1}, {a: 2}, {b: 3}]\n", "one\n{% for i in xs %}\n{% if true %}\nvalue {{ i.a }}\n{% endif %}\n{% endfor %}\n"),
        ("render error, a filter", "x: abc\n", "one\n{{ x | divided_by: 0 }}\n"),
    ];
    for (name, yaml, body) in located {
        let _ = writeln!(summary, "  {name}\n    {}", describe(&render(&parser, body, &scope(yaml))));
    }
    let _ = writeln!(summary, "  a bad tag from a loop's third iteration:");
    let body = "Intro\n\n{% for x in xs %}\n<Card title=\"{{ x }}\" />\n{% endfor %}\n\nOutro\n";
    let g = scope("xs: [one, two, 'th\"ree']\n");
    if let Rendered::Output(out) = render(&parser, body, &g) {
        let parsed = parse_dialect(&out);
        let _ = writeln!(summary, "    rendered: {out:?}\n    {}", describe_parsed(&parsed));
        if let Parsed::Error { reason, line: Some(line), column } = parsed {
            let lines: Vec<&str> = out.lines().collect();
            let snippet = lines.get(line.saturating_sub(1)).copied().unwrap_or("");
            let _ = writeln!(summary, "    mock message:\n      index.mdx, after the Liquid Pass, line {line}: {reason}\n        {snippet}\n      Line numbers refer to the output of the Liquid Pass.\n    source line finder: lines containing the snippet's tag name in the source: {:?}", column.map(|_| body.lines().enumerate().filter(|(_, l)| l.contains("<Card")).map(|(i, _)| i + 1).collect::<Vec<_>>()));
        }
    }

    // Determinism.
    let _ = writeln!(summary, "\nthe clock (values vary, so only the shape is compared):");
    for template in ["{{ \"now\" | date: \"%Y\" }}", "{{ \"today\" | date: \"%Y\" }}", "{{ \"2026-01-02\" | date: \"%Y\" }}", "{{ 'now' }}"] {
        let result = describe(&render(&parser, template, &liquid::Object::new()));
        let shape: String = result.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
        let _ = writeln!(summary, "  {template}: {shape}");
    }
    let replaced = liquid::ParserBuilder::with_stdlib().filter(FixedDate).build().unwrap();
    let _ = writeln!(summary, "  with our own date filter registered after stdlib:");
    for template in ["{{ \"now\" | date: \"%Y\" }}", "{{ \"NOW\" | date: \"%Y\" }}", "{{ \" today \" | date: \"%Y\" }}", "{{ \"13 Jun 2016 02:30:00 +0300\" | date: \"%Y-%m-%d\" }}", "{{ \"2016-06-13 02:30:00 +0300\" | date: \"%a, %b %d, %y\" }}", "{{ \"hello\" | date: \"%Y\" }}", "{{ z | date: \"%Y\" }}", "{{ 5 | date: \"%Y\" }}"] {
        let g = scope("z: null\n");
        let stock = describe(&render(&parser, template, &g));
        let ours = describe(&render(&replaced, template, &g));
        let same = if stock == ours { "same as stock" } else { "differs from stock" };
        let _ = writeln!(summary, "    {template}: {} ({same})", one_line(&ours).chars().map(|c| if template.contains("ow") && c.is_ascii_digit() { '#' } else { c }).collect::<String>());
    }
    let _ = writeln!(summary, "\nfinding the source line for errors with no location, by searching for the name:");
    for (name, yaml, body) in located {
        if let Rendered::Parse(e) | Rendered::Render(e) = render(&parser, body, &scope(yaml)) {
            let wanted = ["requested variable=", "requested filter=", "requested index="].iter().find_map(|k| e.split(k).nth(1)).and_then(|t| t.lines().next()).map(str::to_string);
            let lines: Vec<usize> = wanted.as_ref().map_or(Vec::new(), |w| body.lines().enumerate().filter(|(_, l)| l.contains(w.as_str())).map(|(i, _)| i + 1).collect());
            let _ = writeln!(summary, "  {name}: name {wanted:?}, body lines {lines:?}");
        }
    }

    // Random templates.
    robustness(&parser, &mut summary);

    let out = manifest.join("results").join(label);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("summary.txt"), &summary).unwrap();
    std::fs::write(out.join("detail.txt"), &detail).unwrap();
    print!("{summary}");
}

fn robustness(parser: &liquid::Parser, summary: &mut String) {
    let globals = scope("x: \"hello é\"\nxs: [1, 2, 3]\nn: 3\no: {k: v}\nf: 1.5\nw: \"日本語 text\"\n");
    let inputs = ["x", "xs", "n", "o", "f", "w", "z"];
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut counts: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    let mut panics: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut tally = |name: &'static str, template: String, parser: &liquid::Parser| {
        let entry = counts.entry(name).or_default();
        match render(parser, &template, &globals) {
            Rendered::Output(_) => entry.0 += 1,
            Rendered::Parse(_) | Rendered::Render(_) => entry.1 += 1,
            Rendered::Panic(at) => {
                entry.2 += 1;
                panics.entry(at).or_insert_with(|| (0, template.clone())).0 += 1;
            }
        }
    };
    for _ in 0..100_000 {
        let count = 1 + rng.below(16);
        let template: String = (0..count).map(|_| rng.pick(cases::TEMPLATE_TOKENS)).collect();
        tally("random tokens", template, parser);
    }
    for _ in 0..100_000 {
        let input = rng.pick(&inputs);
        let count = 1 + rng.below(4);
        let chain: Vec<&str> = (0..count).map(|_| rng.pick(cases::FILTERS)).collect();
        tally("filter chains", format!("{{{{ {input} | {} }}}}", chain.join(" | ")), parser);
    }
    let _ = writeln!(summary, "\nrandom templates:");
    for (name, (ok, err, panic_count)) in &counts {
        let _ = writeln!(summary, "  {name}: {} templates: {ok} rendered, {err} errors, {panic_count} caught panics", ok + err + panic_count);
    }
    for (location, (count, input)) in &panics {
        let small = reduce(parser, input, location, &globals);
        let _ = writeln!(summary, "  panic x{count} at {location}: {small:?}");
    }
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
            let parser = parser();
            let globals = scope(cases::data());
            for template in &args[1..] {
                let template = template.replace("\\n", "\n").replace("\\t", "\t");
                println!("{template:?}\n  {}", describe(&render(&parser, &template, &globals)));
            }
        }
        _ => eprintln!("usage: spike-liquid run|deep <label>"),
    }
}
