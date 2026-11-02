//! Spike 1: parse, serialize, parse again, and compare with positions removed.
//!
//! Usage: spike-serializer <label> [--fences] [--resource-link]
//! Writes results/<label>/{summary.txt,misses.txt}.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use markdown::mdast::{Heading, InlineCode, List, ListItem, Node, Paragraph, Root, Text};
use markdown::{
    to_html_with_options, to_mdast, CompileOptions, Constructs, Options as HtmlOptions, ParseOptions,
};
use mdast_util_to_markdown::{to_markdown_with_options, Options};

#[derive(Clone, Copy)]
enum Grammar {
    CommonMark,
    Gfm,
    Dialect,
}

fn parse_options(grammar: Grammar) -> ParseOptions {
    match grammar {
        Grammar::CommonMark => ParseOptions::default(),
        Grammar::Gfm => ParseOptions::gfm(),
        // The custom Constructs from Spike 2's method.
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
                ..Constructs::gfm()
            },
            ..ParseOptions::gfm()
        },
    }
}

fn strip(node: &mut Node) {
    node.position_set(None);
    if let Some(children) = node.children_mut() {
        children.iter_mut().for_each(strip);
    }
}

fn has_position(node: &Node) -> bool {
    node.position().is_some()
        || node
            .children()
            .is_some_and(|children| children.iter().any(has_position))
}

thread_local! {
    static PANIC_MESSAGE: RefCell<String> = const { RefCell::new(String::new()) };
}

fn guarded<T>(work: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(work))
        .map_err(|_| PANIC_MESSAGE.with(|message| message.borrow().clone()))
}

struct Record {
    suite: &'static str,
    name: String,
    input: String,
    kind: &'static str,
    serialized: Option<String>,
    detail: String,
    html_equal: Option<bool>,
}

/// The first point where two debug-printed trees differ, with some context.
fn first_difference(a: &Node, b: &Node) -> String {
    let (a, b) = (format!("{a:?}"), format!("{b:?}"));
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
    format!("before: ...{}\nafter:  ...{}", tail(&a), tail(&b))
}

fn roundtrip_tree(
    suite: &'static str,
    name: String,
    input: String,
    mut tree: Node,
    second: Grammar,
    options: &Options,
) -> Record {
    let mut record = Record {
        suite,
        name,
        input,
        kind: "equal",
        serialized: None,
        detail: String::new(),
        html_equal: None,
    };
    strip(&mut tree);

    let serialized = match guarded(|| to_markdown_with_options(&tree, options)) {
        Err(message) => {
            record.kind = "panic";
            record.detail = message;
            return record;
        }
        Ok(Err(message)) => {
            record.kind = "serialize_err";
            record.detail = message.reason.chars().take(200).collect();
            return record;
        }
        Ok(Ok(serialized)) => serialized,
    };
    record.serialized = Some(serialized.clone());

    let reparsed = match guarded(|| to_mdast(&serialized, &parse_options(second))) {
        Err(message) => {
            record.kind = "panic";
            record.detail = format!("in second parse: {message}");
            return record;
        }
        Ok(Err(message)) => {
            record.kind = "parse_err";
            record.detail = format!("second parse: {message}");
            return record;
        }
        Ok(Ok(reparsed)) => reparsed,
    };
    let mut reparsed = reparsed;
    strip(&mut reparsed);

    if reparsed != tree {
        record.kind = "mismatch";
        record.detail = first_difference(&tree, &reparsed);
    }
    record
}

fn roundtrip(
    suite: &'static str,
    name: String,
    input: &str,
    first: Grammar,
    second: Grammar,
    options: &Options,
) -> Record {
    let failed = |kind, detail| Record {
        suite,
        name: name.clone(),
        input: input.to_string(),
        kind,
        serialized: None,
        detail,
        html_equal: None,
    };
    match guarded(|| to_mdast(input, &parse_options(first))) {
        Err(message) => failed("panic", format!("in first parse: {message}")),
        Ok(Err(message)) => failed("parse_err", format!("first parse: {message}")),
        Ok(Ok(tree)) => {
            let mut record = roundtrip_tree(suite, name.clone(), input.to_string(), tree, second, options);
            // Evidence for "harmless": does the miss still render the same? Only for
            // same-grammar runs, because to_html rejects MDX constructs differently.
            if record.kind == "mismatch" && !matches!(first, Grammar::Dialect) {
                let render = |text: &str| {
                    to_html_with_options(
                        text,
                        &HtmlOptions {
                            parse: parse_options(first),
                            compile: CompileOptions {
                                allow_dangerous_html: true,
                                allow_dangerous_protocol: true,
                                ..CompileOptions::default()
                            },
                        },
                    )
                    .ok()
                };
                record.html_equal = Some(render(input) == render(record.serialized.as_deref().unwrap_or("")));
            }
            record
        }
    }
}

/// What reaches Stage 5: node kinds in Dialect parses, including whether Html can.
fn dialect_node_kinds(inputs: &[String]) -> (usize, BTreeMap<String, usize>) {
    fn visit(node: &Node, kinds: &mut BTreeMap<String, usize>) {
        let debug = format!("{node:?}");
        let kind = debug.split([' ', '(']).next().unwrap_or("").to_string();
        *kinds.entry(kind).or_default() += 1;
        if let Some(children) = node.children() {
            children.iter().for_each(|child| visit(child, kinds));
        }
    }
    let mut parsed = 0;
    let mut kinds = BTreeMap::new();
    for input in inputs {
        if let Ok(Ok(tree)) = guarded(|| to_mdast(input, &parse_options(Grammar::Dialect))) {
            parsed += 1;
            visit(&tree, &mut kinds);
        }
    }
    (parsed, kinds)
}

/// Proves the comparison can see differences and that positions don't survive.
fn controls() {
    let source = "# h\n\n> - a *b*\n>   - c\n\nx[^1]\n\n[^1]: note **n**\n\n| a |\n|---|\n| b |\n\n- [x] t\n";
    let parse = |text: &str| to_mdast(text, &parse_options(Grammar::Gfm)).unwrap();

    let mut a = parse(source);
    let mut b = parse(&format!("\n\n\n{source}"));
    assert_ne!(a, b, "control: positions should differ before stripping");
    strip(&mut a);
    strip(&mut b);
    assert!(!has_position(&a), "control: a position survived stripping");
    assert_eq!(a, b, "control: same text must compare equal once stripped");

    fn alter(node: &mut Node) -> bool {
        if let Node::Text(text) = node {
            if text.value == "n" {
                text.value = "N".into();
                return true;
            }
        }
        node.children_mut()
            .is_some_and(|children| children.iter_mut().any(alter))
    }
    assert!(alter(&mut b), "control: expected text inside the footnote");
    assert_ne!(a, b, "control: an altered footnote child must compare unequal");
}

include!("suites.rs");

fn main() {
    std::panic::set_hook(Box::new(|info| {
        PANIC_MESSAGE.with(|message| *message.borrow_mut() = info.to_string());
    }));
    controls();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let label = args.first().expect("usage: spike-serializer <label> [flags]");
    let mut options = Options::default();
    for flag in &args[1..] {
        match flag.as_str() {
            "--fences" => options.fences = true,
            "--resource-link" => options.resource_link = true,
            other => panic!("unknown flag {other}"),
        }
    }

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifest.join("spec.json")).unwrap()).unwrap();
    let spec = spec.as_array().unwrap();
    assert_eq!(spec.len(), 652, "CommonMark 0.31.2 has 652 examples");

    let mut records = Vec::new();
    for example in spec {
        let input = example["markdown"].as_str().unwrap();
        let name = format!("example {} ({})", example["example"], example["section"].as_str().unwrap());
        records.push(roundtrip("A", name.clone(), input, Grammar::CommonMark, Grammar::CommonMark, &options));
        records.push(roundtrip("B", name, input, Grammar::Gfm, Grammar::Gfm, &options));
    }
    for (name, input) in curated() {
        records.push(roundtrip("C", name.into(), &input, Grammar::Gfm, Grammar::Gfm, &options));
    }
    for (name, input) in dialect_cases() {
        records.push(roundtrip("D", name.into(), input, Grammar::Dialect, Grammar::Gfm, &options));
    }
    for (name, tree) in hand_built() {
        let input = format!("{tree:?}");
        records.push(roundtrip_tree("E", name, input, tree, Grammar::Gfm, &options));
    }

    let mut summary = format!("label: {label}\nargs: {:?}\n\n", &args[1..]);
    let mut misses = String::new();
    let mut outputs = String::new();
    for suite in ["A", "B", "C", "D", "E"] {
        let in_suite: Vec<&Record> = records.iter().filter(|r| r.suite == suite).collect();
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
        for record in &in_suite {
            *kinds.entry(record.kind).or_default() += 1;
            if record.kind == "serialize_err" {
                let reason = record.detail.split('(').next().unwrap_or("").to_string();
                *reasons.entry(reason).or_default() += 1;
            }
            if suite != "A" && suite != "B" {
                let _ = writeln!(outputs, "{suite} | {} | {} | {:?}", record.kind, record.name, record.serialized);
            }
            if record.kind != "equal" {
                let _ = write!(
                    misses,
                    "=== {suite} | {} | {}\ninput: {:?}\nserialized: {:?}\nhtml_equal: {:?}\n{}\n\n",
                    record.kind, record.name, record.input, record.serialized, record.html_equal, record.detail
                );
            }
        }
        let equal = kinds.get("equal").copied().unwrap_or(0);
        let percent = 100.0 * equal as f64 / in_suite.len() as f64;
        let _ = writeln!(summary, "suite {suite}: {equal}/{} equal ({percent:.2}%) {kinds:?}", in_suite.len());
        for (reason, count) in reasons {
            let _ = writeln!(summary, "    {count} x {reason}");
        }
    }

    let mut stage5_inputs: Vec<String> = spec.iter().map(|e| e["markdown"].as_str().unwrap().to_string()).collect();
    stage5_inputs.extend(curated().into_iter().map(|(_, input)| input));
    let (parsed, kinds) = dialect_node_kinds(&stage5_inputs);
    let _ = writeln!(summary, "\ndialect parses: {parsed}/{} node kinds: {kinds:?}", stage5_inputs.len());

    let out = manifest.join("results").join(label);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("summary.txt"), &summary).unwrap();
    std::fs::write(out.join("misses.txt"), &misses).unwrap();
    std::fs::write(out.join("outputs.txt"), &outputs).unwrap();
    print!("{summary}");
}
