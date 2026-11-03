//! Spike 5: does Expansion hold together from a Project to Intermediate Markdown?
//!
//! Usage:
//!   spike-expansion run <label>                Fixtures, generated suites, and robustness
//!   spike-expansion render <label> <binary>    Each Project that expanded, through Spike 4's renderer
//!   spike-expansion one <project dir>          Prints the Intermediate Markdown, or the error
//!   spike-expansion child ...                  Internal

mod expand;
mod serialize;
mod template;
#[path = "../../02-dialect/src/cases.rs"]
#[allow(dead_code)]
mod dialect_cases;

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use expand::{expand, Disk, Files, Memory, Problem};
use liquid::model::Value;
use pulldown_cmark::{CowStr, Event, Tag};
use serialize::parse_markdown;
use template::{Engines, EDGE};

fn manifest() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

enum Expect {
    Markdown(String),
    Error(Vec<String>),
}

struct Fixture {
    group: &'static str,
    name: String,
    flags: Vec<String>,
    files: Vec<(String, String)>,
    expect: Expect,
}

fn end_section(text: &mut String) -> String {
    let body = std::mem::take(text);
    format!("{}\n", body.trim_end_matches('\n'))
}

fn load_fixtures(group: &'static str) -> Vec<Fixture> {
    let path = manifest().join("fixtures").join(format!("{group}.txt"));
    let text = std::fs::read_to_string(path).unwrap();
    let mut fixtures = Vec::new();
    for block in text.split("\n@@@ ").skip(1) {
        let (header, rest) = block.split_once('\n').unwrap();
        let mut words = header.split_whitespace();
        let name = words.next().unwrap().to_string();
        let flags = words.map(str::to_string).collect();
        let mut files = Vec::new();
        let mut expect = None;
        let mut section: Option<String> = None;
        let mut body = String::new();
        let mut close = |section: &mut Option<String>, body: &mut String| {
            if let Some(title) = section.take() {
                let content = end_section(body);
                match title.as_str() {
                    "expect-md" => expect = Some(Expect::Markdown(content)),
                    "expect-error" => expect = Some(Expect::Error(content.lines().map(str::to_string).collect())),
                    _ => files.push((title, content)),
                }
            }
        };
        for line in rest.split('\n') {
            if let Some(title) = line.strip_prefix("--- ") {
                close(&mut section, &mut body);
                section = Some(title.trim().to_string());
            } else {
                body.push_str(line);
                body.push('\n');
            }
        }
        close(&mut section, &mut body);
        fixtures.push(Fixture { group, name, flags, files, expect: expect.expect("every fixture says what it expects") });
    }
    fixtures
}

fn materialize(fixture: &Fixture, root: &Path) -> PathBuf {
    let dir = root.join(fixture.group).join(&fixture.name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, content) in &fixture.files {
        let target = dir.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let mut bytes = content.clone();
        if fixture.flags.iter().any(|f| f == "crlf") {
            bytes = bytes.replace('\n', "\r\n");
        }
        if fixture.flags.iter().any(|f| f == "no-final-newline") {
            bytes.truncate(bytes.trim_end().len());
        }
        if fixture.flags.iter().any(|f| f == "bom") {
            bytes.insert(0, '\u{feff}');
        }
        std::fs::write(target, bytes).unwrap();
    }
    dir
}

/// Footnote labels don't change the document, so number them by first appearance.
fn relabel(events: Vec<Event<'static>>) -> Vec<Event<'static>> {
    let mut names: HashMap<String, String> = HashMap::new();
    let mut name = |label: &str| {
        let next = format!("n{}", names.len());
        CowStr::from(names.entry(label.to_string()).or_insert(next).clone())
    };
    events
        .into_iter()
        .map(|event| match event {
            Event::FootnoteReference(label) => Event::FootnoteReference(name(&label)),
            Event::Start(Tag::FootnoteDefinition(label)) => Event::Start(Tag::FootnoteDefinition(name(&label))),
            other => other,
        })
        .collect()
}

fn same_document(a: &str, b: &str) -> bool {
    relabel(parse_markdown(a)) == relabel(parse_markdown(b))
}

fn outcome_text(result: &Result<String, Problem>) -> String {
    match result {
        Ok(markdown) => format!("MARKDOWN:\n{markdown}"),
        Err(problem) => format!("ERROR{}:\n{problem}\n", if problem.internal { " (internal)" } else { "" }),
    }
}

// ---------------------------------------------------------------------------
// The fixture suites
// ---------------------------------------------------------------------------

#[derive(PartialEq, Clone, Copy)]
enum Verdict {
    Pass,
    /// A document came out, and it was the wrong one, or one that should have been an error.
    SilentlyWrong,
    /// An error came out where a document should have, or an error with the wrong text.
    Other,
}

fn judge(fixture: &Fixture, result: &Result<String, Problem>) -> (Verdict, String) {
    match (&fixture.expect, result) {
        (Expect::Markdown(expected), Ok(actual)) if same_document(actual, expected) => (Verdict::Pass, String::new()),
        (Expect::Markdown(expected), Ok(actual)) => (Verdict::SilentlyWrong, format!("expected the document:\n{expected}got:\n{actual}")),
        (Expect::Markdown(expected), Err(problem)) => (Verdict::Other, format!("expected the document:\n{expected}got the error:\n{problem}")),
        (Expect::Error(wanted), Err(problem)) => {
            let text = problem.to_string();
            let missing: Vec<&String> = wanted.iter().filter(|w| match w.strip_prefix('!') { Some(not) => text.contains(not), None => !text.contains(w.as_str()) }).collect();
            if missing.is_empty() {
                (Verdict::Pass, String::new())
            } else {
                (Verdict::Other, format!("the error didn't satisfy {missing:?}:\n{text}"))
            }
        }
        (Expect::Error(wanted), Ok(actual)) => (Verdict::SilentlyWrong, format!("expected an error with {wanted:?}, got the document:\n{actual}")),
    }
}

fn run_fixture_suites(label: &str, engines: &Engines, summary: &mut String) {
    let out = manifest().join("results").join(label);
    let projects = manifest().join("target").join("projects").join(label);
    let intermediate = out.join("intermediate");
    let _ = std::fs::remove_dir_all(&intermediate);
    std::fs::create_dir_all(&intermediate).unwrap();
    let mut outputs = String::new();
    for group in ["design", "checks", "limits"] {
        let fixtures = load_fixtures(group);
        let mut counts = (0, 0, 0);
        let mut misses = String::new();
        for fixture in &fixtures {
            let dir = materialize(fixture, &projects);
            let result = expand(&Disk(dir), engines, None);
            let (verdict, detail) = judge(fixture, &result);
            let mark = match verdict {
                Verdict::Pass => {
                    counts.0 += 1;
                    "pass"
                }
                Verdict::SilentlyWrong => {
                    counts.1 += 1;
                    "SILENTLY WRONG"
                }
                Verdict::Other => {
                    counts.2 += 1;
                    "MISS"
                }
            };
            if verdict != Verdict::Pass {
                let _ = writeln!(misses, "  {mark} [{group}] {}\n{}", fixture.name, indent(&detail));
            }
            let _ = writeln!(outputs, "=== [{group}] {}: {mark}\n{}", fixture.name, outcome_text(&result));
            if let Ok(markdown) = &result {
                std::fs::write(intermediate.join(format!("{group}-{}.md", fixture.name)), markdown).unwrap();
            }
        }
        let _ = writeln!(summary, "{group}: {} fixtures, {} pass, {} silently wrong, {} other misses", fixtures.len(), counts.0, counts.1, counts.2);
        summary.push_str(&misses);
    }
    std::fs::write(out.join("outputs.txt"), outputs).unwrap();
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("      {l}\n")).collect()
}

// ---------------------------------------------------------------------------
// Hostile values through the whole pipeline
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

/// From Spike 3.
const VALUE_TOKENS: &[&str] = &[
    "a", "b c", " ", "\"", "'", "&", "AT&T", "&amp;", "&#35;", "&b", "<", ">", "<A>", "</A>", "{", "}", "{x}", "{{", "}}", "{%", "%}",
    "\n", "\n\n", "\r\n", "\t", "\\", "é", "日本語", "🍩", "*", "_", "`", "[", "]", "|", "=", "/", "#", "-", "> ", "- ", "    ",
];

/// The document's structure, and the text in it.
fn shape(markdown: &str) -> (String, String) {
    let (mut kinds, mut text) = (String::new(), String::new());
    for event in parse_markdown(markdown) {
        match event {
            Event::Text(t) => text.push_str(&t),
            Event::SoftBreak => text.push('\n'),
            other => {
                let _ = write!(kinds, "{other:?},");
            }
        }
    }
    (kinds, text)
}

fn hostile_values(engines: &Engines, summary: &mut String) {
    let wraps: &[(&str, &str)] = &[
        ("paragraph", "A S B\n"),
        ("heading", "# A S B\n"),
        ("list item", "- A S B\n"),
        ("quote", "> A S B\n"),
        ("table cell", "| h |\n|---|\n| A S B |\n"),
        ("emphasis", "*A S B*\n"),
        ("strong", "**A S B**\n"),
        ("link text", "[A S B](/u)\n"),
        ("footnote", "x[^1]\n\n[^1]: A S B\n"),
    ];
    let component = "---\nprops:\n  type: object\n  properties:\n    v: { type: string }\n---\nA {props.v} B\n";
    let modes: &[(&str, &str, &[(&str, &str)])] = &[
        ("{t}", "{t}", wraps),
        ("{{ t | text }} in a loop", "{% for t in ts %}{{ t | text }}{% endfor %}", wraps),
        (
            "{{ t | attr }} to a Prop to {props.v}",
            "",
            &[
                ("block", "{% for t in ts %}<P v=\"{{ t | attr }}\" />{% endfor %}\n"),
                ("list item", "{% for t in ts %}- <P v=\"{{ t | attr }}\" />{% endfor %}\n"),
                ("quote", "{% for t in ts %}> <P v=\"{{ t | attr }}\" />{% endfor %}\n"),
                ("table cell", "| h |\n|---|\n{% for t in ts %}| <P v=\"{{ t | attr }}\" /> |{% endfor %}\n"),
                ("mid-sentence", "{% for t in ts %}x <P v=\"{{ t | attr }}\" /> y{% endfor %}\n"),
            ],
        ),
    ];
    let _ = writeln!(summary, "\nhostile values, 20,000 per placement, through the whole pipeline. Exact means the same structure and text as a plain value:");
    for (mode, spelled, placements) in modes {
        let _ = writeln!(summary, "  {mode}:");
        for (placement, wrap) in placements.iter() {
            let body = wrap.replace('S', spelled);
            let mut map = BTreeMap::new();
            map.insert("index.mdx".to_string(), body);
            map.insert("components/P.mdx".to_string(), component.to_string());
            let files = Memory(map);
            let run = |value: &str| {
                let mut data = liquid::Object::new();
                data.insert("t".into(), Value::scalar(value.to_string()));
                data.insert("ts".into(), Value::Array(vec![Value::scalar(value.to_string())]));
                catch_unwind(AssertUnwindSafe(|| expand(&files, engines, Some(&data))))
            };
            let (base_kinds, base_text) = shape(&run("PLAIN").unwrap().unwrap());
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
            let (mut exact, mut different, mut panics) = (0, 0, 0);
            let mut errors: BTreeMap<String, (usize, String)> = BTreeMap::new();
            let mut first_different = None;
            for _ in 0..20_000 {
                let count = 1 + rng.below(6);
                let value: String = (0..count).map(|_| rng.pick(VALUE_TOKENS)).collect();
                let trimmed = value.trim_matches(EDGE);
                if trimmed.is_empty() {
                    continue;
                }
                match run(&value) {
                    Err(_) => panics += 1,
                    Ok(Ok(markdown)) => {
                        let (kinds, text) = shape(&markdown);
                        if kinds == base_kinds && text == base_text.replace("PLAIN", trimmed) {
                            exact += 1;
                        } else {
                            different += 1;
                            first_different.get_or_insert((value.clone(), markdown));
                        }
                    }
                    Ok(Err(problem)) => {
                        let key: String = problem.message.split(':').next().unwrap_or("").chars().take(70).collect();
                        errors.entry(key).or_insert((0, value.clone())).0 += 1;
                    }
                }
            }
            let error_total: usize = errors.values().map(|e| e.0).sum();
            let _ = writeln!(summary, "    {placement}: {exact} exact, {different} silently different, {error_total} errors, {panics} escaped panics");
            for (key, (count, value)) in &errors {
                let _ = writeln!(summary, "      error x{count}: {key}, first {value:?}");
            }
            if let Some((value, markdown)) = first_different {
                let _ = writeln!(summary, "      first different: {value:?} gave {markdown:?}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Random Projects
// ---------------------------------------------------------------------------

const PROJECT_TOKENS: &[&str] = &[
    "<A>", "</A>", "<B>", "</B>", "<C>", "</C>", "<A />", "<B />", "<C />", "<D />", "<E />", "<F />", "<G />", "<L />", "<M />", "<M v=\"p\" />", "<Content />",
    "{x}", "{y | upcase}", "{{ x }}", "{{ x | text }}", "{{ y }}", "{% if x %}", "{% endif %}", "{%- if x %}", "{%- endif %}", "{% for i in xs %}", "{% endfor %}", "{{ i }}", "{{ i | text }}",
    "> ", "- ", "1. ", "# ", "## ", "| a |", "| a | b |", "|---|", "|---|---|", "\n", "\n", "\n\n", "\n\n", "**", "*", "`", "~~", "```\n", "[l](/u)", "[r]", "\n[r]: /v\n", "[^1]", "\n[^1]: n\n", "[", "](/u)", "![a](i.png)",
    "word", "two words", " ", "  ", "\\", "\\\n", "  \n", "<", ">", "{", "}", "&amp;", "---", "===", "é", "日本語",
];

fn random_project_files() -> Memory {
    let mut map = BTreeMap::new();
    for (name, body) in [
        ("A", "> <Content />\n"),
        ("B", "- <Content />\n- b\n"),
        ("C", "| h |\n|---|\n| <Content /> |\n"),
        ("D", "**d**\n"),
        ("E", "one\n\ntwo\n"),
        ("F", "{% if false %}x{% endif %}\n"),
        ("G", "g[^1]\n\n[^1]: gn\n"),
        ("L", "[l](/l)\n"),
        ("M", "---\nprops:\n  type: object\n  properties:\n    v: { type: string, default: dv }\n---\n{props.v}\n"),
    ] {
        map.insert(format!("components/{name}.mdx"), body.to_string());
    }
    Memory(map)
}

fn random_projects(engines: &Engines, summary: &mut String) {
    let mut files = random_project_files();
    let mut data = liquid::Object::new();
    data.insert("x".into(), Value::scalar("va*l"));
    data.insert("y".into(), Value::scalar("two words"));
    data.insert("xs".into(), Value::Array(vec![Value::scalar("a"), Value::scalar("b")]));
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let (mut expanded, mut panics, mut html, mut unstable) = (0, 0, 0, 0);
    let mut document: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut internal: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut unrepresentable: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let total = 100_000;
    for round in 0..total {
        let count = 1 + rng.below(24);
        let body: String = (0..count).map(|_| rng.pick(PROJECT_TOKENS)).collect();
        files.0.insert("index.mdx".into(), body.clone());
        let run = catch_unwind(AssertUnwindSafe(|| expand(&files, engines, Some(&data))));
        match run {
            Err(_) => panics += 1,
            Ok(Ok(markdown)) => {
                expanded += 1;
                if parse_markdown(&markdown).iter().any(|e| matches!(e, Event::Html(_) | Event::InlineHtml(_))) {
                    html += 1;
                    internal.entry("raw HTML in the Intermediate Markdown".into()).or_insert((0, body.clone())).0 += 1;
                }
                if round % 10 == 0 && expand(&files, engines, Some(&data)).ok().as_deref() != Some(markdown.as_str()) {
                    unstable += 1;
                }
            }
            Ok(Err(problem)) => {
                let cleaned: String = problem.message.split(['`', ':']).next().unwrap_or("").chars().take(60).collect();
                let key = if cleaned.is_empty() { problem.message.chars().take(60).collect() } else { cleaned };
                if problem.message.contains("represented exactly") {
                    let detail = problem.message.split("Markdown: ").nth(1).unwrap_or("");
                    let detail: String = detail.split(['"', '(']).next().unwrap_or("").chars().take(60).collect();
                    unrepresentable.entry(detail).or_insert((0, body)).0 += 1;
                } else if problem.internal {
                    internal.entry(key).or_insert((0, body)).0 += 1;
                } else {
                    document.entry(key).or_insert((0, body)).0 += 1;
                }
            }
        }
    }
    let _ = writeln!(summary, "\nrandom Projects, {total} documents built from tags, Expressions, Liquid, and Markdown:");
    let _ = writeln!(summary, "  {expanded} expanded and round-tripped, {} document errors, {} unrepresentable, {} internal, {panics} escaped panics, {html} with raw HTML, {unstable} changed on a second run", document.values().map(|v| v.0).sum::<usize>(), unrepresentable.values().map(|v| v.0).sum::<usize>(), internal.values().map(|v| v.0).sum::<usize>());
    for (title, map, limit) in [("internal", &internal, 20), ("unrepresentable", &unrepresentable, 20), ("document errors, most common", &document, 12)] {
        let _ = writeln!(summary, "  {title}:");
        let mut sorted: Vec<_> = map.iter().collect();
        sorted.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
        for (key, (count, example)) in sorted.into_iter().take(limit) {
            let _ = writeln!(summary, "    x{count} {key}\n        first: {example:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// The parser, with and without a final line ending
// ---------------------------------------------------------------------------

fn parser_robustness(summary: &mut String) {
    let options = expand::dialect_options();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut total, mut raw, mut with_newline) = (0, 0, 0);
    let mut first = None;
    let mut check = |input: String| {
        total += 1;
        if catch_unwind(AssertUnwindSafe(|| markdown::to_mdast(&input, &options))).is_err() {
            raw += 1;
            first.get_or_insert_with(|| input.clone());
        }
        let ended = if input.ends_with('\n') { input } else { format!("{input}\n") };
        if catch_unwind(AssertUnwindSafe(|| markdown::to_mdast(&ended, &options))).is_err() {
            with_newline += 1;
        }
    };
    for _ in 0..100_000 {
        let count = 1 + rng.below(14);
        check((0..count).map(|_| rng.pick(dialect_cases::TOKENS)).collect());
    }
    for _ in 0..100_000 {
        let count = 1 + rng.below(30);
        check((0..count).map(|_| rng.pick(dialect_cases::DOC_TOKENS)).collect());
    }
    let _ = writeln!(summary, "\nthe fork, on Spike 2's random tokens with the callback on, {total} documents: {raw} caught panics as written, first {first:?}, and {with_newline} with a final line ending added");
}

// ---------------------------------------------------------------------------
// Child processes: determinism, depth, and scale
// ---------------------------------------------------------------------------

fn run_child(args: &[&str], timeout: Duration) -> String {
    let exe = std::env::current_exe().unwrap();
    let mut process = Command::new(exe).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let start = Instant::now();
    loop {
        match process.try_wait().unwrap() {
            Some(status) => {
                let output = process.wait_with_output().unwrap();
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                return if status.success() { text } else { format!("ABORT ({status}) after: {text}") };
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

fn single_file(body: String) -> Memory {
    let mut map = BTreeMap::new();
    map.insert("index.mdx".to_string(), body);
    map
        .insert("components/A.mdx".to_string(), "<Content />\n".to_string());
    Memory(map)
}

fn describe_timed(files: &dyn Files, engines: &Engines) -> String {
    let start = Instant::now();
    let result = catch_unwind(AssertUnwindSafe(|| expand(files, engines, None)));
    let seconds = start.elapsed().as_secs_f64();
    match result {
        Err(_) => format!("escaped panic after {seconds:.3}s"),
        Ok(Ok(markdown)) => format!("{} bytes of Markdown in {seconds:.3}s", markdown.len()),
        Ok(Err(problem)) => format!("error in {seconds:.3}s: {}", problem.message.chars().take(90).collect::<String>()),
    }
}

fn child(args: &[String], engines: &Engines) {
    match args[0].as_str() {
        "depth" => {
            let n: usize = args[2].parse().unwrap();
            let body = match args[1].as_str() {
                "quote" => format!("{}x\n", "> ".repeat(n)),
                "list" => format!("{}x\n", "- ".repeat(n)),
                "tag" => format!("{}x\n{}", "<A>\n".repeat(n), "</A>\n".repeat(n)),
                "emphasis" => format!("{}x{}\n", "*a _b ".repeat(n), " b_ a*".repeat(n)),
                "if" => format!("{}x{}\n", "{% if true %}".repeat(n), "{% endif %}".repeat(n)),
                "for" => format!("{}x{}\n", "{% for i in (1..1) %}".repeat(n), "{% endfor %}".repeat(n)),
                other => panic!("unknown shape {other}"),
            };
            println!("{}", describe_timed(&single_file(body), engines));
        }
        "scale" => {
            let n: usize = args[2].parse().unwrap();
            let body = match args[1].as_str() {
                "table" => format!("| a | b |\n|---|---|\n{{%- for i in (1..{n}) %}}\n| {{{{ i }}}} | value |\n{{%- endfor %}}\n"),
                "prose" => format!("{{% for i in (1..{n}) %}}Line {{{{ i }}}} of ordinary prose.\n\n{{% endfor %}}"),
                "components" => format!("{{% for i in (1..{n}) %}}<A>Line {{{{ i }}}}.</A>\n\n{{% endfor %}}"),
                other => panic!("unknown shape {other}"),
            };
            println!("{}", describe_timed(&single_file(body), engines));
        }
        other => panic!("unknown child {other}"),
    }
}

fn robustness(summary: &mut String) {
    let _ = writeln!(summary, "\ndeep nesting, in a process of its own with a 1 GiB stack. It must give an error or a document, and never abort:");
    for (shape, n) in [("quote", 200), ("quote", 5_000), ("list", 200), ("list", 5_000), ("tag", 200), ("tag", 5_000), ("emphasis", 200), ("emphasis", 5_000), ("if", 5_000), ("if", 10_000), ("for", 5_000), ("for", 10_000)] {
        let result = run_child(&["child", "depth", shape, &n.to_string()], Duration::from_secs(60));
        let _ = writeln!(summary, "  {shape:>8} {n:>6}: {result}");
    }
    let _ = writeln!(summary, "\nscale:");
    for (shape, sizes) in [("table", [1_000usize, 4_000, 16_000, 32_000]), ("prose", [1_000, 4_000, 16_000, 32_000]), ("components", [100, 400, 1_600, 6_400])] {
        for n in sizes {
            let result = run_child(&["child", "scale", shape, &n.to_string()], Duration::from_secs(180));
            let _ = writeln!(summary, "  {shape:>10} {n:>6}: {result}");
            if result.starts_with("timeout") {
                break;
            }
        }
    }
}

fn determinism(label: &str, summary: &mut String) {
    let _ = writeln!(summary, "\ndeterminism, each Project in 6 separate processes:");
    let projects = manifest().join("target").join("projects").join(label);
    for (group, name) in [("design", "object-order-is-sorted"), ("design", "prd-example"), ("design", "footnote-in-a-component-used-twice")] {
        let dir = projects.join(group).join(name);
        let mut distinct: Vec<String> = (0..6).map(|_| run_child(&["one", dir.to_str().unwrap()], Duration::from_secs(20))).collect();
        distinct.sort();
        distinct.dedup();
        let _ = writeln!(summary, "  {name}: {} distinct output(s)", distinct.len());
    }
}

fn odd_inputs(label: &str, engines: &Engines, summary: &mut String) {
    let _ = writeln!(summary, "\nodd Projects:");
    let root = manifest().join("target").join("projects").join(label).join("odd");
    let _ = std::fs::remove_dir_all(&root);
    let empty = root.join("no-index");
    std::fs::create_dir_all(&empty).unwrap();
    let not_text = root.join("not-utf8");
    std::fs::create_dir_all(&not_text).unwrap();
    std::fs::write(not_text.join("index.mdx"), [0x66, 0xff, 0xfe, 0x0a]).unwrap();
    let bad_component = root.join("component-not-utf8");
    std::fs::create_dir_all(bad_component.join("components")).unwrap();
    std::fs::write(bad_component.join("index.mdx"), "<A />\n").unwrap();
    std::fs::write(bad_component.join("components/A.mdx"), [0x66, 0xff, 0xfe, 0x0a]).unwrap();
    for dir in [empty, not_text, bad_component] {
        let name = dir.file_name().unwrap().to_str().unwrap().to_string();
        let _ = writeln!(summary, "  {name}: {}", outcome_text(&expand(&Disk(dir), engines, None)).replace('\n', " "));
    }
}

fn controls(engines: &Engines) {
    assert!(catch_unwind(|| panic!("deliberate")).is_err(), "control: the guard must catch a panic");
    assert!(same_document("> a\n", " > a\n"), "control: spacing must not count as a difference");
    assert!(!same_document("a *b*\n", "a b\n"), "control: emphasis must count as a difference");
    assert!(same_document("x[^a]\n\n[^a]: n\n", "x[^b]\n\n[^b]: n\n"), "control: footnote labels must not count");
    let mut map = BTreeMap::new();
    map.insert("index.mdx".to_string(), "---\nt: \"*x*\"\n---\n{t}\n".to_string());
    assert_eq!(expand(&Memory(map), engines, None).unwrap().trim(), "\\*x\\*", "control: an Expression must print as data");
    let outcome = run_child(&["child", "abortme"], Duration::from_secs(10));
    assert!(outcome.starts_with("ABORT"), "control: a failing child must be reported, got {outcome}");
}

// ---------------------------------------------------------------------------
// The renderer
// ---------------------------------------------------------------------------

fn tool(name: &str, args: &[&str]) -> String {
    let output = Command::new(name).args(args).output().unwrap_or_else(|e| panic!("{name}: {e}"));
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn sha256(path: &Path) -> String {
    tool("sha256sum", &[path.to_str().unwrap()]).split_whitespace().next().unwrap_or("").to_string()
}

fn squeeze(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn render_all(label: &str, binary: &str) {
    let out = manifest().join("results").join(label);
    let projects = manifest().join("target").join("projects").join(label);
    let scratch = manifest().join("target").join("pdf").join(label);
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    let mut report = String::new();
    let (mut compiled, mut texts_ok, mut same, mut total) = (0, 0, 0, 0);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(out.join("intermediate")).unwrap().map(|e| e.unwrap().path()).collect();
    paths.sort();
    let photo = manifest().parent().unwrap().join("04-typst/fixtures/assets/photo.png");
    for path in paths {
        total += 1;
        let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
        let (group, name) = stem.split_once('-').unwrap();
        let project = projects.join(group).join(name);
        if name == "image-path-passes-through" {
            std::fs::create_dir_all(project.join("sub")).unwrap();
            std::fs::copy(&photo, project.join("chart.png")).unwrap();
            std::fs::copy(&photo, project.join("sub/x.png")).unwrap();
        }
        // Success criterion 4: written into the Project and rendered as a Markdown Input.
        let inside = project.join("emitted.md");
        std::fs::copy(&path, &inside).unwrap();
        let first = scratch.join(format!("{stem}-inside.pdf"));
        let second = scratch.join(format!("{stem}-elsewhere.pdf"));
        let render = |input: &Path, pdf: &Path, cwd: &Path| {
            Command::new(binary).current_dir(cwd).env("SPIKE_MECHANISM", "literal").args(["render", input.to_str().unwrap(), pdf.to_str().unwrap(), "--assets", project.to_str().unwrap()]).stderr(Stdio::piped()).output().unwrap()
        };
        let one = render(&inside, &first, &project);
        let two = render(&path, &second, &scratch);
        if !one.status.success() {
            let message: String = String::from_utf8_lossy(&one.stderr).lines().take(2).collect::<Vec<_>>().join(" ").chars().take(220).collect();
            let _ = writeln!(report, "  DID NOT COMPILE {stem}: {message}");
            continue;
        }
        compiled += 1;
        let matches_hash = two.status.success() && sha256(&first) == sha256(&second);
        same += usize::from(matches_hash);
        if !matches_hash {
            let _ = writeln!(report, "  DIFFERENT PDF {stem}");
        }
        let markdown = std::fs::read_to_string(&path).unwrap();
        let text = squeeze(&tool("pdftotext", &["-layout", first.to_str().unwrap(), "-"]));
        let wanted: Vec<String> = parse_markdown(&markdown).into_iter().filter_map(|e| match e { Event::Text(t) | Event::Code(t) => Some(squeeze(&t)), _ => None }).filter(|t| !t.is_empty()).collect();
        let missing: Vec<&String> = wanted.iter().filter(|w| !text.contains(w.as_str())).collect();
        if missing.is_empty() {
            texts_ok += 1;
        } else {
            let _ = writeln!(report, "  TEXT MISSING {stem}: {missing:?}");
        }
    }
    let head = format!("rendered through Spike 4's binary, with string literals. {total} Projects that expanded: {compiled} compiled, {texts_ok} with all their text in pdftotext, {same} with the same PDF rendered inside the Project and elsewhere\n");
    let full = format!("{head}{report}");
    std::fs::write(out.join("render.txt"), &full).unwrap();
    print!("{full}");
}

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Valid documents, with markers that show whether content moved
// ---------------------------------------------------------------------------

struct Writer {
    rng: Rng,
    marker: usize,
}

impl Writer {
    fn mark(&mut self) -> String {
        self.marker += 1;
        format!("w{:05}w", self.marker)
    }

    fn atom(&mut self, inline_only: bool) -> String {
        let mark = self.mark();
        match self.rng.below(if inline_only { 20 } else { 22 }) {
            0..=4 => mark,
            5 => format!("*{mark}*"),
            6 => format!("**{mark}**"),
            7 => format!("`{mark}`"),
            8 => format!("~~{mark}~~"),
            9 => format!("[{mark}](/u)"),
            10 => format!("{mark} ![a](i.png)"),
            11 => format!("{mark} {{x}}"),
            12 => format!("{mark} {{y | upcase}}"),
            13 => format!("{mark} {{{{ x | text }}}}"),
            14 => format!("{mark} <D />"),
            15 => format!("{mark} <F />"),
            16 => format!("{mark} <M v=\"p\" />"),
            17 => format!("<K>{mark}</K>"),
            18 => format!("{mark}[^1]"),
            19 => format!("{mark} \\* &amp; é"),
            20 => format!("{mark} <G />"),
            _ => format!("<R /> {mark}"),
        }
    }

    fn line(&mut self, inline_only: bool) -> String {
        let count = 1 + self.rng.below(4);
        // Text first, so a line doesn't become all tags, or start with something block-like.
        let mut out = vec![self.mark()];
        for _ in 0..count {
            out.push(self.atom(inline_only));
        }
        out.join(" ")
    }

    fn blocks(&mut self, depth: usize) -> Vec<String> {
        let count = 1 + self.rng.below(3);
        let mut out = Vec::new();
        for index in 0..count {
            if index > 0 {
                out.push(String::new());
            }
            out.extend(self.block(depth));
        }
        out
    }

    fn block(&mut self, depth: usize) -> Vec<String> {
        let nested = |writer: &mut Self, first: &str, rest: &str, depth: usize| -> Vec<String> {
            let lines = writer.blocks(depth - 1);
            lines
                .into_iter()
                .enumerate()
                .map(|(i, line)| match (i, line.is_empty()) {
                    (0, _) => format!("{first}{line}"),
                    (_, true) => String::new(),
                    _ => format!("{rest}{line}"),
                })
                .collect()
        };
        let choice = self.rng.below(if depth == 0 { 8 } else { 16 });
        match choice {
            0..=2 => vec![self.line(false)],
            3 => vec![self.line(false), self.line(false)],
            4 => vec![format!("## {}", self.line(true))],
            5 => vec!["```".to_string(), format!("{} {{x}} <A />", self.mark()), "```".to_string()],
            6 => {
                let header = self.mark();
                // An Expression's pipe needs a backslash in a cell. Liquid's doesn't, because
                // Liquid has already gone by the time the table is read.
                let cell = self.line(true).replace("{y | upcase}", "{y \\| upcase}");
                vec![format!("| {header} | b |"), "|---|---|".to_string(), format!("| {cell} | c |")]
            }
            7 => vec![format!("<{} />", ["D", "E", "F", "L", "T", "M"][self.rng.below(6)])],
            8 => {
                let lines = self.blocks(depth - 1);
                lines.into_iter().map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") }).collect()
            }
            9 => {
                let mut out = nested(self, "- ", "  ", depth);
                out.extend(nested(self, "- ", "  ", depth));
                out
            }
            10 => {
                let mut out = nested(self, "1. ", "   ", depth);
                out.extend(nested(self, "2. ", "   ", depth));
                out
            }
            11 => vec![format!("- [x] {}", self.line(false))],
            _ => {
                let name = ["A", "B", "Q", "N"][self.rng.below(4)];
                let mut out = vec![format!("<{name}>")];
                out.extend(self.blocks(depth - 1));
                out.push(format!("</{name}>"));
                out
            }
        }
    }
}

fn valid_project_files() -> Memory {
    let mut map = BTreeMap::new();
    for (name, body) in [
        ("A", "> <Content />\n"),
        ("B", "- <Content />\n- b\n"),
        ("Q", "Head\n\n<Content />\n\nFoot\n"),
        ("N", "<Content />\n"),
        ("K", "**<Content />**\n"),
        ("D", "**d**\n"),
        ("E", "one\n\ntwo\n"),
        ("F", "{% if false %}x{% endif %}\n"),
        ("G", "g[^1]\n\n[^1]: gn\n"),
        ("L", "- l1\n- l2\n"),
        ("T", "| t |\n|---|\n| v |\n"),
        ("R", "[l](/l)\n"),
        ("M", "---\nprops:\n  type: object\n  properties:\n    v: { type: string, default: dv }\n---\n{props.v}\n"),
    ] {
        map.insert(format!("components/{name}.mdx"), body.to_string());
    }
    Memory(map)
}

/// Removes characters while the same problem keeps happening.
fn reduce(input: &str, same: &dyn Fn(&str) -> bool) -> String {
    let mut chars: Vec<char> = input.chars().collect();
    let mut size = chars.len().max(2) / 2;
    while size >= 1 {
        let mut start = 0;
        while start + size <= chars.len() {
            let mut candidate = chars.clone();
            candidate.drain(start..start + size);
            if same(&candidate.iter().collect::<String>()) {
                chars = candidate;
            } else {
                start += 1;
            }
        }
        size /= 2;
    }
    chars.into_iter().collect()
}

fn problem_key(problem: &Problem) -> String {
    let message = &problem.message;
    if let Some(detail) = message.split("Markdown: ").nth(1) {
        return format!("unrepresentable: {}", detail.split(['"', '(']).next().unwrap_or("").chars().take(60).collect::<String>());
    }
    message.split(['`', ':']).next().unwrap_or("").chars().take(60).collect()
}

fn valid_documents(engines: &Engines, summary: &mut String) {
    let mut files = valid_project_files();
    let mut data = liquid::Object::new();
    data.insert("x".into(), Value::scalar("va*l"));
    data.insert("y".into(), Value::scalar("two words"));
    let mut writer = Writer { rng: Rng(0xDEAD_BEEF_1234_5678), marker: 0 };
    let total = 100_000;
    let (mut expanded, mut lost, mut moved, mut panics) = (0, 0, 0, 0);
    let mut problems: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut first_lost = None;
    let mut first_moved = None;
    for _ in 0..total {
        writer.marker = 0;
        let depth = writer.rng.below(4);
        let mut body = writer.blocks(depth).join("\n");
        body.push_str("\n\n[^1]: note\n");
        let count = writer.marker;
        files.0.insert("index.mdx".into(), body.clone());
        match catch_unwind(AssertUnwindSafe(|| expand(&files, engines, Some(&data)))) {
            Err(_) => panics += 1,
            Ok(Err(problem)) => {
                problems.entry(problem_key(&problem)).or_insert((0, body)).0 += 1;
            }
            Ok(Ok(markdown)) => {
                expanded += 1;
                let seen: Vec<usize> = markdown
                    .split('w')
                    .filter_map(|piece| (piece.len() == 5).then(|| piece.parse().ok()).flatten())
                    .collect();
                let mut sorted = seen.clone();
                sorted.sort_unstable();
                let complete = sorted.len() == count && sorted.iter().enumerate().all(|(i, m)| *m == i + 1);
                if !complete {
                    lost += 1;
                    first_lost.get_or_insert_with(|| (body.clone(), markdown.clone()));
                } else if seen != sorted {
                    moved += 1;
                    first_moved.get_or_insert_with(|| (body.clone(), markdown.clone()));
                }
            }
        }
    }
    let _ = writeln!(summary, "\nvalid documents, {total} built to be correct, each word carrying a marker:");
    let _ = writeln!(summary, "  {expanded} expanded, {} refused, {panics} escaped panics, {lost} with a marker lost or repeated, {moved} with markers out of order", problems.values().map(|p| p.0).sum::<usize>());
    if let Some((body, markdown)) = first_lost {
        let _ = writeln!(summary, "  first lost or repeated: {body:?}\n    gave {markdown:?}");
    }
    if let Some((body, markdown)) = first_moved {
        let _ = writeln!(summary, "  first out of order: {body:?}\n    gave {markdown:?}");
    }
    let mut sorted: Vec<_> = problems.iter().collect();
    sorted.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
    for (key, (count, example)) in sorted {
        // Reducing may wander to a different cause with the same message, so it stays with
        // inputs that still expand when the problem's own text is removed from them.
        let same = |text: &str| {
            let mut scratch = valid_project_files();
            scratch.0.insert("index.mdx".into(), text.to_string());
            matches!(catch_unwind(AssertUnwindSafe(|| expand(&scratch, engines, Some(&data)))), Ok(Err(p)) if problem_key(&p) == *key)
        };
        let mut scratch = valid_project_files();
        scratch.0.insert("index.mdx".into(), example.clone());
        let message = expand(&scratch, engines, Some(&data)).err().map_or(String::new(), |p| p.to_string());
        let _ = writeln!(summary, "  x{count} {key}\n      first: {example:?}\n      says: {:?}\n      reduced: {:?}", message.chars().take(240).collect::<String>(), reduce(example, &same));
    }
}

fn run_all(label: &str, engines: &Engines) {
    controls(engines);
    let out = manifest().join("results").join(label);
    std::fs::create_dir_all(&out).unwrap();
    let mut summary = format!("label: {label}\n\n");
    run_fixture_suites(label, engines, &mut summary);
    hostile_values(engines, &mut summary);
    random_projects(engines, &mut summary);
    valid_documents(engines, &mut summary);
    parser_robustness(&mut summary);
    odd_inputs(label, engines, &mut summary);
    determinism(label, &mut summary);
    robustness(&mut summary);
    std::fs::write(out.join("summary.txt"), &summary).unwrap();
    print!("{summary}");
}

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The whole pipeline runs on a large stack, as the design says, because Liquid's and
    // our own recursive walks would otherwise abort on deep input, past any guard.
    let work = std::thread::Builder::new().stack_size(1 << 30).spawn(move || {
        let engines = Engines::new();
        match args.first().map(String::as_str) {
            Some("run") => run_all(&args[1], &engines),
            Some("fixtures") => {
                let mut summary = String::new();
                std::fs::create_dir_all(manifest().join("results").join(&args[1])).unwrap();
                run_fixture_suites(&args[1], &engines, &mut summary);
                print!("{summary}");
            }
            Some("valid") => {
                let mut summary = String::new();
                valid_documents(&engines, &mut summary);
                print!("{summary}");
            }
            Some("render") => render_all(&args[1], &args[2]),
            Some("one") => {
                let result = expand(&Disk(PathBuf::from(&args[1])), &engines, None);
                print!("{}", outcome_text(&result));
            }
            Some("child") if args[1] == "abortme" => std::process::exit(3),
            Some("child") => child(&args[1..], &engines),
            _ => eprintln!("usage: spike-expansion run|render|one ..."),
        }
    });
    work.unwrap().join().unwrap();
}
