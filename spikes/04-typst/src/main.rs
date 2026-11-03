//! Spike 4: can Typst do the Renderer's job in-process, offline, and deterministically?
//!
//! Usage:
//!   spike-typst run <label>                         Fixtures, fuzz, and robustness
//!   spike-typst render <in.md> <out.pdf> [--page f] [--fonts dir] [--assets dir]
//!   spike-typst deep <label>                        Nesting and timing in child processes
//!   spike-typst child ...                           Internal

mod cases;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options as MdOptions, Parser, Tag, TagEnd};
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Smart};
use typst::layout::{Frame, FrameItem};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::FontStore;
use typst_layout::PagedDocument;
use typst_pdf::PdfOptions;

thread_local! {
    static PANIC_LOCATION: RefCell<String> = const { RefCell::new(String::new()) };
}

// ---------------------------------------------------------------------------
// The World
// ---------------------------------------------------------------------------

static LIBRARY: LazyLock<LazyHash<Library>> = LazyLock::new(|| LazyHash::new(Library::default()));

fn store(fonts_dir: Option<&Path>) -> FontStore {
    let mut store = FontStore::new();
    store.extend(typst_kit::fonts::embedded());
    if let Some(dir) = fonts_dir {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
        paths.sort();
        for path in paths {
            let data = std::fs::read(&path).unwrap();
            store.extend(Font::iter(Bytes::new(data)).map(|f| {
                let info = f.info().clone();
                (f, info)
            }));
        }
    }
    store
}

static EMBEDDED: LazyLock<FontStore> = LazyLock::new(|| store(None));

struct SpikeWorld<'a> {
    fonts: &'a FontStore,
    main: FileId,
    source: Source,
    root: Option<PathBuf>,
    reads: Mutex<Vec<String>>,
}

impl<'a> SpikeWorld<'a> {
    fn new(fonts: &'a FontStore, text: String, root: Option<PathBuf>) -> Self {
        let main = FileId::new(RootedPath::new(VirtualRoot::Project, VirtualPath::new("/main.typ").unwrap()));
        Self { fonts, main, source: Source::new(main, text), root, reads: Mutex::new(Vec::new()) }
    }
}

impl World for SpikeWorld<'_> {
    fn library(&self) -> &LazyHash<Library> {
        &LIBRARY
    }

    fn book(&self) -> &LazyHash<FontBook> {
        self.fonts.book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main {
            Ok(self.source.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }

    /// Only files inside the root, after resolving symlinks.
    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let requested = id.get().vpath().get_with_slash().to_string();
        self.reads.lock().unwrap().push(requested.clone());
        let root = self.root.as_ref().ok_or(FileError::AccessDenied)?;
        let root = root.canonicalize().map_err(|_| FileError::AccessDenied)?;
        let path = root.join(requested.trim_start_matches('/'));
        let real = path.canonicalize().map_err(|_| FileError::NotFound(path.clone()))?;
        if !real.starts_with(&root) {
            self.reads.lock().unwrap().push(format!("DENIED {requested}"));
            return Err(FileError::AccessDenied);
        }
        std::fs::read(&real).map(Bytes::new).map_err(|_| FileError::NotFound(path))
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.font(index)
    }

    fn today(&self, _offset: Option<typst::foundations::Duration>) -> Option<Datetime> {
        Datetime::from_ymd(2000, 1, 1)
    }
}

struct Compiled {
    document: Result<PagedDocument, String>,
    warnings: Vec<String>,
    reads: Vec<String>,
}

fn compile(fonts: &FontStore, source: String, root: Option<PathBuf>) -> Compiled {
    let world = SpikeWorld::new(fonts, source, root);
    let warned = typst::compile::<PagedDocument>(&world);
    let warnings = warned.warnings.iter().map(|w| w.message.to_string()).collect();
    let document = warned.output.map_err(|errors| errors.iter().map(|e| e.message.to_string()).collect::<Vec<_>>().join("; "));
    let reads = world.reads.lock().unwrap().clone();
    Compiled { document, warnings, reads }
}

fn to_pdf(document: &PagedDocument) -> Result<Vec<u8>, String> {
    let options = PdfOptions { ident: Smart::Auto, timestamp: None, ..PdfOptions::default() };
    typst_pdf::pdf(document, &options).map_err(|errors| errors.iter().map(|e| e.message.to_string()).collect::<Vec<_>>().join("; "))
}

// ---------------------------------------------------------------------------
// Laid-out pages
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Laid {
    text: String,
    missing: usize,
    sizes: Vec<f64>,
}

fn walk(frame: &Frame, out: &mut Laid) {
    for (_, item) in frame.items() {
        match item {
            FrameItem::Group(group) => walk(&group.frame, out),
            FrameItem::Text(text) => {
                out.text.push_str(&text.text);
                out.text.push(' ');
                out.missing += text.glyphs.iter().filter(|g| g.id == 0).count();
                out.sizes.push(text.size.to_pt());
            }
            _ => {}
        }
    }
}

fn laid_out(document: &PagedDocument) -> Laid {
    let mut out = Laid::default();
    for page in document.pages() {
        walk(&page.frame, &mut out);
    }
    out
}

fn squeeze(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

// ---------------------------------------------------------------------------
// The emitter
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Mechanism {
    /// Escape every ASCII punctuation character in markup.
    Table,
    /// Write the text as a string literal, `#"..."`.
    Literal,
}

fn escape_table(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn string_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn emit_text(text: &str, mechanism: Mechanism) -> String {
    match mechanism {
        Mechanism::Table => escape_table(text),
        Mechanism::Literal => format!("#{};", string_literal(text)),
    }
}

struct Emitter<'a> {
    events: Vec<Event<'a>>,
    index: usize,
    mechanism: Mechanism,
    notes: HashMap<String, usize>,
    note_ranges: Vec<(usize, usize)>,
    emitted_notes: HashSet<usize>,
    /// Notes being written now, so one that references itself can't refer to a label
    /// that doesn't exist yet.
    open_notes: HashSet<usize>,
    warnings: Vec<String>,
    title: Option<String>,
    errors: Vec<String>,
    last_list_loose: bool,
}

fn collect_notes(events: &[Event]) -> (HashMap<String, usize>, Vec<(usize, usize)>) {
    let mut notes = HashMap::new();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::FootnoteDefinition(label)) = &events[i] {
            let start = i + 1;
            let mut depth = 0;
            let mut j = start;
            loop {
                match &events[j] {
                    Event::Start(Tag::FootnoteDefinition(_)) => depth += 1,
                    Event::End(TagEnd::FootnoteDefinition) if depth == 0 => break,
                    Event::End(TagEnd::FootnoteDefinition) => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            if !notes.contains_key(label.as_ref()) {
                notes.insert(label.to_string(), ranges.len());
                ranges.push((start, j));
            }
            i = j;
        }
        i += 1;
    }
    (notes, ranges)
}

fn plain_text(events: &[Event], index: &mut usize) -> String {
    let mut depth = 0usize;
    let mut text = String::new();
    while *index < events.len() {
        match &events[*index] {
            Event::Start(_) => depth += 1,
            Event::End(_) if depth == 0 => return text,
            Event::End(_) => depth -= 1,
            Event::Text(t) | Event::Code(t) => text.push_str(t),
            Event::SoftBreak | Event::HardBreak => text.push(' '),
            _ => {}
        }
        *index += 1;
    }
    text
}

impl Emitter<'_> {
    /// Emits until the matching end event, which it consumes.
    fn inner(&mut self) -> String {
        let mut out = String::new();
        while self.index < self.events.len() {
            let event = self.events[self.index].clone();
            self.index += 1;
            match event {
                Event::End(_) => return out,
                Event::Start(tag) => self.start(tag, &mut out),
                Event::Text(text) => out.push_str(&emit_text(&text, self.mechanism)),
                Event::Code(code) => {
                    let _ = write!(out, "#raw({});", string_literal(&code));
                }
                Event::SoftBreak => out.push(' '),
                Event::HardBreak => out.push_str("#linebreak();"),
                Event::Rule => out.push_str("#line(length: 100%)\n\n"),
                Event::TaskListMarker(done) => {
                    let glyph = if done { "\u{2611} " } else { "\u{2610} " };
                    let _ = write!(out, "#{};", string_literal(glyph));
                }
                Event::FootnoteReference(label) => self.footnote(&label, &mut out),
                Event::Html(html) | Event::InlineHtml(html) => self.warnings.push(format!("skipped raw HTML: {html:?}")),
                Event::InlineMath(t) | Event::DisplayMath(t) => out.push_str(&emit_text(&t, self.mechanism)),
            }
        }
        out
    }

    fn footnote(&mut self, label: &str, out: &mut String) {
        let Some(&number) = self.notes.get(label) else {
            out.push_str(&emit_text(&format!("[^{label}]"), self.mechanism));
            return;
        };
        if self.emitted_notes.insert(number) {
            let (start, _) = self.note_ranges[number];
            let saved = self.index;
            self.index = start;
            self.open_notes.insert(number);
            let body = self.inner_until_note_end();
            self.open_notes.remove(&number);
            self.index = saved;
            let _ = write!(out, "#[#footnote[{body}]<fn-{number}>];");
        } else if self.open_notes.contains(&number) {
            out.push_str(&emit_text(&format!("[^{label}]"), self.mechanism));
        } else {
            let _ = write!(out, "#footnote(<fn-{number}>);");
        }
    }

    fn inner_until_note_end(&mut self) -> String {
        self.inner()
    }

    fn start(&mut self, tag: Tag, out: &mut String) {
        match tag {
            Tag::Paragraph => {
                let body = self.inner();
                out.push_str(&body);
                out.push_str("\n\n");
            }
            Tag::Heading { level, .. } => {
                if self.title.is_none() && level as usize == 1 {
                    let mut i = self.index;
                    self.title = Some(plain_text(&self.events, &mut i));
                }
                let body = self.inner();
                let _ = write!(out, "#heading(level: {})[{body}]\n\n", level as usize);
            }
            Tag::BlockQuote(_) => {
                let body = self.inner();
                let _ = write!(out, "#quote(block: true)[{body}]\n\n");
            }
            Tag::CodeBlock(kind) => {
                let mut code = String::new();
                while let Event::Text(t) = &self.events[self.index] {
                    code.push_str(t);
                    self.index += 1;
                }
                self.index += 1;
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info.split_whitespace().next().unwrap_or("").to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                let lang = if lang.is_empty() { "none".to_string() } else { string_literal(&lang) };
                let _ = write!(out, "#raw(block: true, lang: {lang}, {})\n\n", string_literal(code.strip_suffix('\n').unwrap_or(&code)));
            }
            Tag::List(start) => {
                let items = self.list_items();
                let tight = !self.last_list_loose;
                match start {
                    None => {
                        let _ = write!(out, "#list(tight: {tight}, {items})\n\n");
                    }
                    Some(n) => {
                        let _ = write!(out, "#enum(start: {n}, tight: {tight}, {items})\n\n");
                    }
                }
            }
            Tag::Item => unreachable!("items are handled by list_items"),
            Tag::Table(aligns) => self.table(&aligns, out),
            Tag::Emphasis => {
                let body = self.inner();
                let _ = write!(out, "#emph[{body}];");
            }
            Tag::Strong => {
                let body = self.inner();
                let _ = write!(out, "#strong[{body}];");
            }
            Tag::Strikethrough => {
                let body = self.inner();
                let _ = write!(out, "#strike[{body}];");
            }
            Tag::Link { dest_url, .. } => {
                let body = self.inner();
                if dest_url.is_empty() {
                    out.push_str(&body);
                } else {
                    let _ = write!(out, "#link({})[{body}];", string_literal(&dest_url));
                }
            }
            Tag::Image { dest_url, .. } => {
                let mut i = self.index;
                let alt = plain_text(&self.events, &mut i);
                let mut depth = 0;
                loop {
                    match &self.events[self.index] {
                        Event::Start(_) => depth += 1,
                        Event::End(_) if depth == 0 => break,
                        Event::End(_) => depth -= 1,
                        _ => {}
                    }
                    self.index += 1;
                }
                self.index += 1;
                if dest_url.contains(':') && !dest_url.starts_with('.') {
                    self.errors.push(format!("Remote and absolute images aren't supported: {dest_url}"));
                } else if dest_url.is_empty() {
                    out.push_str(&emit_text(&alt, self.mechanism));
                } else {
                    let _ = write!(out, "#box(image({}, alt: {}));", string_literal(&dest_url), string_literal(&alt));
                }
            }
            Tag::FootnoteDefinition(_) => {
                // Placed at its first reference.
                let mut depth = 0;
                loop {
                    match &self.events[self.index] {
                        Event::Start(_) => depth += 1,
                        Event::End(_) if depth == 0 => break,
                        Event::End(_) => depth -= 1,
                        _ => {}
                    }
                    self.index += 1;
                }
                self.index += 1;
            }
            Tag::HtmlBlock => {
                let _ = self.inner();
            }
            other => {
                let _ = self.inner();
                self.warnings.push(format!("unexpected tag {other:?}"));
            }
        }
    }
}

impl Emitter<'_> {
    fn list_items(&mut self) -> String {
        let mut items = Vec::new();
        let mut loose = false;
        loop {
            let event = self.events[self.index].clone();
            self.index += 1;
            match event {
                Event::Start(Tag::Item) => {
                    if matches!(self.events[self.index], Event::Start(Tag::Paragraph)) {
                        loose = true;
                    }
                    let body = self.inner();
                    items.push(format!("[{body}]"));
                }
                _ => break,
            }
        }
        self.last_list_loose = loose;
        items.join(", ")
    }

    fn table(&mut self, aligns: &[Alignment], out: &mut String) {
        let columns = aligns.len();
        let align: Vec<&str> = aligns
            .iter()
            .map(|a| match a {
                Alignment::None | Alignment::Left => "left",
                Alignment::Center => "center",
                Alignment::Right => "right",
            })
            .collect();
        let mut head = Vec::new();
        let mut rows = Vec::new();
        loop {
            let event = self.events[self.index].clone();
            self.index += 1;
            match event {
                Event::Start(Tag::TableHead) => head = self.cells(),
                Event::Start(Tag::TableRow) => rows.push(self.cells()),
                _ => break,
            }
        }
        let _ = write!(out, "#table(columns: {columns}, align: ({},), table.header({}),", align.join(", "), head.join(", "));
        for mut row in rows {
            row.resize(columns, "[]".to_string());
            let _ = write!(out, " {},", row.join(", "));
        }
        out.push_str(")\n\n");
    }

    fn cells(&mut self) -> Vec<String> {
        let mut cells = Vec::new();
        loop {
            let event = self.events[self.index].clone();
            self.index += 1;
            match event {
                Event::Start(Tag::TableCell) => {
                    let body = self.inner();
                    cells.push(format!("[{body}]"));
                }
                _ => return cells,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Page Settings
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct Page {
    paper: Option<String>,
    flipped: bool,
    width: Option<String>,
    height: Option<String>,
    margins: BTreeMap<String, String>,
    font: Option<String>,
    size: Option<String>,
    numbers: bool,
}

fn valid_length(value: &str) -> bool {
    let number = value.strip_suffix("mm").or_else(|| value.strip_suffix("cm")).or_else(|| value.strip_suffix("pt")).or_else(|| value.strip_suffix("in"));
    number.is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

fn parse_page(text: &str, book: &FontBook) -> Result<Page, String> {
    let mut page = Page::default();
    let mut section = String::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if !["paper", "margins", "type", "page_numbers"].contains(&name) {
                return Err(format!("unknown section [{name}]"));
            }
            section = name.to_string();
            continue;
        }
        let (key, value) = line.split_once('=').ok_or(format!("bad line: {line}"))?;
        let (key, value) = (key.trim(), value.trim());
        let text_value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'));
        let needed = text_value.ok_or(format!("{key} needs a quoted value"));
        let length = |v: Result<&str, String>| -> Result<String, String> {
            let v = v?;
            if valid_length(v) { Ok(v.to_string()) } else { Err(format!("{key}: invalid length {v:?}")) }
        };
        match (section.as_str(), key) {
            ("paper", "size") => match needed? {
                "a4" => page.paper = Some("a4".into()),
                "letter" => page.paper = Some("us-letter".into()),
                other => return Err(format!("unknown paper size {other:?}")),
            },
            ("paper", "orientation") => match needed? {
                "portrait" => page.flipped = false,
                "landscape" => page.flipped = true,
                other => return Err(format!("unknown orientation {other:?}")),
            },
            ("paper", "width") => page.width = Some(length(needed)?),
            ("paper", "height") => page.height = Some(length(needed)?),
            ("margins", side @ ("top" | "right" | "bottom" | "left")) => {
                page.margins.insert(side.to_string(), length(needed)?);
            }
            ("type", "font") => {
                let name = needed?;
                // The book's family names are lowercase.
                if !book.contains_family(&name.to_lowercase()) {
                    return Err(format!("unknown font {name:?}"));
                }
                page.font = Some(name.to_string());
            }
            ("type", "size") => page.size = Some(length(needed)?),
            ("page_numbers", "enabled") => page.numbers = value == "true",
            (section, key) => return Err(format!("unknown key {key} in [{section}]")),
        }
    }
    Ok(page)
}

fn preamble(page: &Page, title: Option<&str>) -> String {
    let mut out = String::new();
    let paper = page.paper.as_deref().unwrap_or("a4");
    let mut args = match (&page.width, &page.height) {
        (Some(w), Some(h)) => format!("width: {w}, height: {h}"),
        _ => format!("paper: \"{paper}\""),
    };
    if page.flipped {
        args.push_str(", flipped: true");
    }
    if !page.margins.is_empty() {
        let sides: Vec<String> = page.margins.iter().map(|(k, v)| format!("{k}: {v}")).collect();
        let _ = write!(args, ", margin: ({})", sides.join(", "));
    } else {
        args.push_str(", margin: 25mm");
    }
    if page.numbers {
        args.push_str(", numbering: \"1\"");
    }
    let _ = writeln!(out, "#set page({args})");
    let font = page.font.as_deref().unwrap_or("Libertinus Serif");
    let size = page.size.as_deref().unwrap_or("11pt");
    let _ = writeln!(out, "#set text(font: {}, size: {size})", string_literal(font));
    let date = "date: none";
    match title {
        Some(title) => {
            let _ = writeln!(out, "#set document(title: {}, {date})", string_literal(title));
        }
        None => {
            let _ = writeln!(out, "#set document({date})");
        }
    }
    out
}

struct Emitted {
    source: String,
    warnings: Vec<String>,
    errors: Vec<String>,
}

fn emit(markdown: &str, page: &Page, mechanism: Mechanism) -> Emitted {
    let flags = MdOptions::ENABLE_TABLES | MdOptions::ENABLE_FOOTNOTES | MdOptions::ENABLE_STRIKETHROUGH | MdOptions::ENABLE_TASKLISTS;
    let events: Vec<Event> = Parser::new_ext(markdown, flags).collect();
    let (notes, note_ranges) = collect_notes(&events);
    let mut emitter = Emitter { events, index: 0, mechanism, notes, note_ranges, emitted_notes: HashSet::new(), open_notes: HashSet::new(), warnings: Vec::new(), title: None, errors: Vec::new(), last_list_loose: false };
    let body = emitter.document();
    Emitted { source: format!("{}{}", preamble(page, emitter.title.as_deref()), body), warnings: emitter.warnings, errors: emitter.errors }
}

impl Emitter<'_> {
    fn document(&mut self) -> String {
        // inner() stops at an end event, and the document has no unmatched one.
        self.inner()
    }
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

fn tool(name: &str, args: &[&str]) -> String {
    let output = Command::new(name).args(args).output().unwrap_or_else(|e| panic!("{name}: {e}"));
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn pdftotext(pdf: &Path) -> String {
    tool("pdftotext", &["-layout", pdf.to_str().unwrap(), "-"])
}

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

fn run_child(args: &[&str], timeout: Duration) -> String {
    let exe = std::env::current_exe().unwrap();
    let mut process = Command::new(exe).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let start = Instant::now();
    loop {
        match process.try_wait().unwrap() {
            Some(status) => {
                let output = process.wait_with_output().unwrap();
                let text = String::from_utf8_lossy(&output.stdout).trim().replace('\n', " ⏎ ");
                return if status.success() { text } else { format!("ABORT ({status}) after: {text}") };
            }
            None if start.elapsed() > timeout => {
                let _ = process.kill();
                let output = process.wait_with_output().unwrap();
                let text = String::from_utf8_lossy(&output.stdout).trim().replace('\n', " ⏎ ");
                return format!("timeout (over {}s) after: {text}", timeout.as_secs());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// A Markdown string whose text event is exactly `text`.
fn md_escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn long_document() -> String {
    let mut out = String::from("# A Long Document\n\n");
    let sentence = "The quick brown fox jumps over the lazy dog, and a report needs paragraphs like this to fill its pages with something to measure. ";
    for section in 1..=78 {
        let _ = write!(out, "## Section {section}\n\n");
        for _ in 0..3 {
            let _ = write!(out, "{}\n\n", sentence.repeat(3));
        }
        out.push_str("- first point about this section\n- second point, with **strong** text\n  - a nested point\n\n");
        out.push_str("| Name | Value |\n|---|--:|\n| alpha | 1 |\n| beta | 2 |\n| gamma | 3 |\n\n");
        out.push_str("```rust\nfn section() -> u32 { 42 }\n```\n\n");
        let _ = write!(out, "> A quote in section {section}, for variety.\n\nA note here[^n{section}].\n\n[^n{section}]: Footnote for section {section}.\n\n");
    }
    out
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

fn controls(root: &Path) {
    let page = Page::default();
    let render = |md: &str| {
        let emitted = emit(md, &page, Mechanism::Table);
        compile(&EMBEDDED, emitted.source, Some(root.to_path_buf()))
    };
    // An unescaped dollar must be seen to change the text, so the fuzz can catch it.
    let raw = compile(&EMBEDDED, "abc $x + y$ def".into(), None);
    let laid = laid_out(&raw.document.expect("control: raw source compiles"));
    assert!(!squeeze(&laid.text).contains("$x+y$"), "control: unescaped math must not print its dollars");
    let safe = render("abc \\$x + y\\$ def\n");
    assert!(squeeze(&laid_out(&safe.document.unwrap()).text).contains("$x+y$"), "control: the emitter must keep dollars");
    // A character no embedded font has must count as missing.
    let cjk = render("你好\n");
    assert!(laid_out(&cjk.document.unwrap()).missing > 0 || !EMBEDDED.book().contains_family("x"), "control: missing glyphs must be seen");
    // Different documents must differ, and the same one must compile.
    let a = to_pdf(&render("one\n").document.unwrap()).unwrap();
    let b = to_pdf(&render("two\n").document.unwrap()).unwrap();
    assert_ne!(a, b, "control: different documents must give different bytes");
    assert!(catch_unwind(|| panic!("deliberate")).is_err(), "control: the guard must catch a panic");
    let outcome = run_child(&["child", "abort"], Duration::from_secs(10));
    assert!(outcome.starts_with("ABORT"), "control: an aborting child must be reported, got {outcome}");
}

// ---------------------------------------------------------------------------
// The suites
// ---------------------------------------------------------------------------

fn write_pdf(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(format!("{name}.pdf"));
    std::fs::write(&path, bytes).unwrap();
    path
}

fn run_suites(label: &str) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let assets = manifest.join("fixtures/assets");
    controls(&assets);
    let out_dir = manifest.join("results").join(label);
    let pdf_dir = out_dir.join("pdf");
    std::fs::create_dir_all(&pdf_dir).unwrap();
    let mut summary = format!("label: {label}\n\n");
    let page = Page::default();

    // Structure.
    let emitted = emit(cases::STRUCTURE, &page, Mechanism::Table);
    std::fs::write(out_dir.join("structure.typ"), &emitted.source).unwrap();
    let compiled = compile(&EMBEDDED, emitted.source.clone(), Some(assets.clone()));
    match &compiled.document {
        Err(e) => {
            let _ = writeln!(summary, "structure: COMPILE ERROR {e}");
        }
        Ok(document) => {
            let pdf = write_pdf(&pdf_dir, "structure", &to_pdf(document).unwrap());
            let text = squeeze(&pdftotext(&pdf));
            let missing: Vec<&&str> = cases::STRUCTURE_EXPECTED.iter().filter(|s| !text.contains(&squeeze(s))).collect();
            let laid = laid_out(document);
            let _ = writeln!(summary, "structure: {} pages, {} of {} strings found, missing {missing:?}, {} missing glyphs, warnings {:?}", document.pages().len(), cases::STRUCTURE_EXPECTED.len() - missing.len(), cases::STRUCTURE_EXPECTED.len(), laid.missing, compiled.warnings);
            let _ = writeln!(summary, "  pdfinfo: {}", tool("pdfinfo", &[pdf.to_str().unwrap()]).lines().filter(|l| l.starts_with("Title") || l.starts_with("Creat") || l.starts_with("Producer") || l.starts_with("ModDate") || l.starts_with("Tagged") || l.starts_with("Pages")).map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(" | "));
            let _ = writeln!(summary, "  links: {:?}", tool("pdfinfo", &["-url", pdf.to_str().unwrap()]).lines().skip(1).map(|l| l.split_whitespace().last().unwrap_or("").to_string()).collect::<Vec<_>>());
            let _ = writeln!(summary, "  fonts: {:?}", tool("pdffonts", &[pdf.to_str().unwrap()]).lines().skip(2).map(|l| l.split_whitespace().next().unwrap_or("").to_string()).collect::<Vec<_>>());
            let bookmarks = tool("pdfinfo", &["-struct", pdf.to_str().unwrap()]);
            let _ = writeln!(summary, "  struct tree: {} lines, H1 tags: {}", bookmarks.lines().count(), bookmarks.lines().filter(|l| l.contains("H1")).count());
        }
    }

    // Adversarial text, in each position, through Markdown.
    let _ = writeln!(summary, "\nadversarial text, each string in each position, through Markdown, then pdftotext:");
    let mut lost: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let positions: &[(&str, &dyn Fn(&str) -> String)] = &[
        ("paragraph", &|m| format!("{m}\n\n")),
        ("heading", &|m| format!("## {m}\n\n")),
        ("list item", &|m| format!("- {m}\n\n")),
        ("table cell", &|m| format!("| h |\n|---|\n| {m} |\n\n")),
        ("quote", &|m| format!("> {m}\n\n")),
        ("emphasis", &|m| format!("*{m}*\n\n")),
        ("strong", &|m| format!("**{m}**\n\n")),
        ("link text", &|m| format!("[{m}](/u)\n\n")),
        ("footnote", &|m| format!("x[^n]\n\n[^n]: {m}\n\n")),
    ];
    let mut total = 0;
    for (mech_name, mechanism, position, wrap) in positions.iter().flat_map(|(p, w)| [("escape table", Mechanism::Table, *p, *w), ("string literal", Mechanism::Literal, *p, *w)]) {
        for sample in cases::ADVERSARIAL {
            total += 1;
            let body = wrap(&format!("Zqmark{} Zqend", md_escape(sample)));
            let emitted = emit(&body, &page, mechanism);
            let result = compile(&EMBEDDED, emitted.source, None);
            let found = match result.document {
                Ok(document) => {
                    let name = "probe";
                    let pdf = write_pdf(&pdf_dir, name, &to_pdf(&document).unwrap());
                    let pages_ok = document.pages().len() == 1;
                    pages_ok && squeeze(&pdftotext(&pdf)).contains(&squeeze(&format!("Zqmark{sample}Zqend")))
                }
                Err(_) => false,
            };
            if !found {
                lost.entry(format!("{mech_name}, {position}")).or_default().push(sample);
            }
        }
    }
    let _ = std::fs::remove_file(pdf_dir.join("probe.pdf"));
    let _ = writeln!(summary, "  {} of {total} found unchanged", total - lost.values().map(Vec::len).sum::<usize>());
    for (position, samples) in &lost {
        let _ = writeln!(summary, "  LOST in {position}: {samples:?}");
    }

    // Other positions: they must compile, and URLs must survive.
    let _ = writeln!(summary, "\nstrings in code, URLs, and alt text:");
    for (name, md) in [
        ("language name", "```a\"b)#read(\"x\")\ncode\n```\n"),
        ("code with quotes and slashes", "```\n\"\\n\" #read(\"x\") $x$ \\\n```\n"),
        ("inline code with a quote and slash", "`\" \\ #x`\n"),
        ("url with a quote", "[t](</a\"b>)\n"),
        ("url with a backslash", "[t](/a\\\\b)\n"),
        ("url with a parenthesis", "[t](</a)b>)\n"),
        ("empty url", "[t]()\n"),
        ("alt text with a quote", "![a\"b\\c](photo.png)\n"),
        ("title with a quote", "# a \"b\" \\c\n"),
    ] {
        let emitted = emit(md, &page, Mechanism::Table);
        let result = compile(&EMBEDDED, emitted.source, Some(assets.clone()));
        let note = match result.document {
            Ok(document) => {
                let pdf = write_pdf(&pdf_dir, "probe", &to_pdf(&document).unwrap());
                let url = tool("pdfinfo", &["-url", pdf.to_str().unwrap()]).lines().skip(1).map(|l| l.split_whitespace().last().unwrap_or("").to_string()).collect::<Vec<_>>();
                let title = tool("pdfinfo", &[pdf.to_str().unwrap()]).lines().find(|l| l.starts_with("Title")).unwrap_or("").to_string();
                format!("compiles, text {:?}, links {url:?}, {title}", pdftotext(&pdf).trim())
            }
            Err(e) => format!("ERROR {e}"),
        };
        let _ = writeln!(summary, "  {name}: {note}");
    }
    let _ = std::fs::remove_file(pdf_dir.join("probe.pdf"));

    // Scripts.
    let _ = writeln!(summary, "\nscripts, embedded fonts only:");
    let script_page = cases::SCRIPTS.iter().map(|(_, s)| format!("{s}\n\n")).collect::<String>();
    let emitted = emit(&script_page, &page, Mechanism::Table);
    if let Ok(document) = compile(&EMBEDDED, emitted.source, None).document {
        write_pdf(&pdf_dir, "scripts", &to_pdf(&document).unwrap());
    }
    let extra_dir = std::env::var("SPIKE_EXTRA_FONTS").ok().map(PathBuf::from);
    let extra = extra_dir.as_ref().map(|d| store(Some(d)));
    for (name, sample) in cases::SCRIPTS {
        for (store_name, fonts) in [("embedded", Some(&*EMBEDDED)), ("fonts dir", extra.as_ref())] {
            let Some(fonts) = fonts else { continue };
            let emitted = emit(&format!("{sample}\n"), &page, Mechanism::Table);
            let result = compile(fonts, emitted.source, None);
            let document = result.document.unwrap();
            let pdf = write_pdf(&pdf_dir, "probe", &to_pdf(&document).unwrap());
            let found = squeeze(&pdftotext(&pdf)).contains(&squeeze(sample));
            let laid = laid_out(&document);
            let _ = writeln!(summary, "  {name:>13} [{store_name:>9}]: pdftotext finds it: {found}, {} missing glyphs, warnings {:?}", laid.missing, result.warnings);
        }
    }
    if let Some(fonts) = &extra {
        let emitted = emit(&script_page, &page, Mechanism::Table);
        if let Ok(document) = compile(fonts, emitted.source, None).document {
            write_pdf(&pdf_dir, "scripts-fonts-dir", &to_pdf(&document).unwrap());
        }
    }
    let _ = std::fs::remove_file(pdf_dir.join("probe.pdf"));

    // Page Settings.
    let _ = writeln!(summary, "\npage settings:");
    let sample = "# Page Test\n\nSome body text, for measuring the margin and font.\n";
    for (name, toml, width, height, left, font, numbered) in cases::PAGES {
        let parsed = parse_page(toml, EMBEDDED.book()).unwrap();
        let emitted = emit(sample, &parsed, Mechanism::Table);
        let document = compile(&EMBEDDED, emitted.source, None).document.unwrap();
        let pdf = write_pdf(&pdf_dir, &format!("page-{}", name.replace(' ', "-")), &to_pdf(&document).unwrap());
        let info = tool("pdfinfo", &[pdf.to_str().unwrap()]);
        let size_line = info.lines().find(|l| l.starts_with("Page size")).unwrap_or("").to_string();
        let parts: Vec<f64> = size_line.split_whitespace().filter_map(|t| t.parse().ok()).collect();
        let size_ok = parts.len() >= 2 && (parts[0] - width).abs() < 0.6 && (parts[1] - height).abs() < 0.6;
        let bbox = tool("pdftotext", &["-bbox", pdf.to_str().unwrap(), "-"]);
        let first_x: f64 = bbox.lines().find(|l| l.contains("<word")).and_then(|l| l.split("xMin=\"").nth(1)).and_then(|t| t.split('"').next()).and_then(|t| t.parse().ok()).unwrap_or(-1.0);
        let left_ok = (first_x - left).abs() < 2.0;
        let fonts = tool("pdffonts", &[pdf.to_str().unwrap()]);
        let font_ok = fonts.contains(font);
        let text = pdftotext(&pdf);
        let last = text.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        let number_ok = (last == "1") == *numbered;
        let laid = laid_out(&document);
        let largest = laid.sizes.iter().cloned().fold(0.0, f64::max);
        let smallest = laid.sizes.iter().cloned().fold(f64::MAX, f64::min);
        let _ = writeln!(summary, "  {name:>13}: size {} ({size_line}), left margin {} (x={first_x:.2}), font {}, numbers {}, body size {smallest:.1}pt, largest {largest:.1}pt", ok(size_ok), ok(left_ok), ok(font_ok), ok(number_ok));
    }
    let _ = writeln!(summary, "invalid settings (all must be errors):");
    for toml in cases::BAD_PAGES {
        let _ = writeln!(summary, "  {toml:?}: {:?}", parse_page(toml, EMBEDDED.book()).map(|_| "ACCEPTED"));
    }

    // Images.
    let _ = writeln!(summary, "\nimages, with the asset root at fixtures/assets:");
    for (name, md, should) in cases::IMAGES {
        let emitted = emit(md, &page, Mechanism::Table);
        let (compiled_ok, message, reads) = if let Some(e) = emitted.errors.first() {
            (false, format!("emitter: {e}"), Vec::new())
        } else {
            let result = compile(&EMBEDDED, emitted.source, Some(assets.clone()));
            let reads = result.reads.clone();
            match result.document {
                Ok(document) => {
                    let pdf = to_pdf(&document);
                    (pdf.is_ok(), format!("warnings {:?}", result.warnings), reads)
                }
                Err(e) => (false, format!("error: {e}"), reads),
            }
        };
        let mark = if compiled_ok == *should { "as expected" } else { "SURPRISE" };
        let _ = writeln!(summary, "  {name}: compiled {compiled_ok} ({mark}), {message}, reads {reads:?}");
    }

    let emitted = emit(cases::STARTED_LIST, &page, Mechanism::Table);
    let document = compile(&EMBEDDED, emitted.source, None).document.unwrap();
    let pdf = write_pdf(&pdf_dir, "started-list", &to_pdf(&document).unwrap());
    let _ = writeln!(summary, "\nstarted list: {:?}", pdftotext(&pdf).split_whitespace().collect::<Vec<_>>().join(" "));

    // Long document.
    let emitted = emit(&long_document(), &page, Mechanism::Table);
    let document = compile(&EMBEDDED, emitted.source, None).document.unwrap();
    let pdf_bytes = to_pdf(&document).unwrap();
    let pdf = write_pdf(&pdf_dir, "long", &pdf_bytes);
    let text = squeeze(&pdftotext(&pdf));
    let _ = writeln!(summary, "\nlong document: {} pages, {} KB, section 78 found: {}, footnote 78 found: {}, {} missing glyphs", document.pages().len(), pdf_bytes.len() / 1024, text.contains("Section78"), text.contains("Footnoteforsection78."), laid_out(&document).missing);

    // Escaping fuzz.
    fuzz(&mut summary, &pdf_dir);

    // Random Markdown.
    random_documents(&mut summary, Mechanism::Table, "escape table", &assets);
    random_documents(&mut summary, Mechanism::Literal, "string literal", &assets);

    std::fs::write(out_dir.join("summary.txt"), &summary).unwrap();
    print!("{summary}");
}

fn ok(value: bool) -> &'static str {
    if value { "ok" } else { "WRONG" }
}

fn fuzz(summary: &mut String, pdf_dir: &Path) {
    let _ = writeln!(summary, "\nescaping fuzz, 20,000 strings per mechanism, checked on the laid-out pages:");
    let containers: &[(&str, fn(&str) -> String)] = &[
        ("paragraph", |t| format!("{t}\n\n")),
        ("heading", |t| format!("#heading(level: 2)[{t}]\n\n")),
        ("emphasis", |t| format!("#emph[{t}];\n\n")),
        ("strong", |t| format!("#strong[{t}];\n\n")),
        ("list", |t| format!("#list([{t}])\n\n")),
        ("table", |t| format!("#table(columns: 1, [{t}])\n\n")),
        ("quote", |t| format!("#quote(block: true)[{t}]\n\n")),
        ("link", |t| format!("#link(\"/u\")[{t}];\n\n")),
        ("footnote", |t| format!("#footnote[{t}];\n\n")),
    ];
    let preamble = "#set document(date: none)\n#set page(width: 40cm, height: auto)\n";
    for (name, mechanism) in [("escape table", Mechanism::Table), ("string literal", Mechanism::Literal)] {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut lost: Vec<String> = Vec::new();
        let mut batch_failures: Vec<String> = Vec::new();
        let mut checked = 0;
        let mut compile_errors = 0;
        let mut panics = 0;
        for batch in 0..200 {
            let mut source = preamble.to_string();
            let mut samples = Vec::new();
            for i in 0..100 {
                let count = 1 + rng.below(6);
                let sample: String = (0..count).map(|_| rng.pick(cases::FUZZ_TOKENS)).collect();
                let (_, wrap) = containers[(batch + i) % containers.len()];
                let marked = format!("Zq{batch}x{i}q {sample} Zr{batch}x{i}r");
                source.push_str(&wrap(&emit_text(&marked, mechanism)));
                samples.push((format!("Zq{batch}x{i}q"), sample, format!("Zr{batch}x{i}r")));
            }
            let result = catch_unwind(AssertUnwindSafe(|| compile(&EMBEDDED, source, None)));
            let Ok(result) = result else {
                panics += 1;
                continue;
            };
            let document = match result.document {
                Ok(d) => d,
                Err(e) => {
                    compile_errors += 1;
                    batch_failures.push(e.chars().take(120).collect());
                    continue;
                }
            };
            let text = squeeze(&laid_out(&document).text);
            for (start, sample, end) in samples {
                checked += 1;
                let wanted = format!("{start}{}{end}", squeeze(&sample));
                if !text.contains(&wanted) {
                    lost.push(sample);
                }
            }
            typst::comemo::evict(1);
        }
        let _ = writeln!(summary, "  {name}: {} of {checked} unchanged, {compile_errors} batches failed to compile, {panics} panics", checked - lost.len());
        // Which tokens are behind the misses?
        let mut by_token: BTreeMap<&str, usize> = BTreeMap::new();
        for sample in &lost {
            for token in cases::FUZZ_TOKENS {
                if sample.contains(token) {
                    *by_token.entry(token).or_default() += 1;
                }
            }
        }
        let mut sorted: Vec<_> = by_token.into_iter().collect();
        sorted.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        if !lost.is_empty() {
            let shortest = lost.iter().min_by_key(|s| s.chars().count()).unwrap();
            let _ = writeln!(summary, "    shortest miss {shortest:?}; tokens in misses: {:?}", sorted.iter().take(8).collect::<Vec<_>>());
            let mut shortest_list: Vec<&String> = lost.iter().collect();
            shortest_list.sort_by_key(|s| s.chars().count());
            shortest_list.dedup();
            let _ = writeln!(summary, "    first few: {:?}", shortest_list.iter().take(8).collect::<Vec<_>>());
        }
        for failure in batch_failures.iter().take(3) {
            let _ = writeln!(summary, "    compile error: {failure}");
        }
    }
    // A sample through the real extraction path.
    let mut rng = Rng(0xABCD_EF01_2345_6789);
    let mut md = String::new();
    let mut samples = Vec::new();
    for i in 0..300 {
        let count = 1 + rng.below(5);
        let sample: String = (0..count).map(|_| rng.pick(cases::FUZZ_TOKENS)).filter(|t| !matches!(*t, "\u{a0}" | "\u{ad}" | "\u{200b}" | "\u{2028}" | "\t" | "ﬁ")).collect();
        let _ = write!(md, "Zq{i}q{}Zr{i}r\n\n", md_escape(&sample));
        samples.push((i, sample));
    }
    let emitted = emit(&md, &Page { width: Some("60cm".into()), height: Some("120cm".into()), ..Page::default() }, Mechanism::Table);
    let document = compile(&EMBEDDED, emitted.source, None).document.unwrap();
    let pdf = write_pdf(pdf_dir, "fuzz-sample", &to_pdf(&document).unwrap());
    let text = squeeze(&pdftotext(&pdf));
    let lost: Vec<&String> = samples.iter().filter(|(i, s)| !text.contains(&format!("Zq{i}q{}Zr{i}r", squeeze(s)))).map(|(_, s)| s).collect();
    let _ = writeln!(summary, "  through Markdown and pdftotext, escape table: {} of {} unchanged, {} of the misses contain no CJK, which bundled fonts lack: {:?}", samples.len() - lost.len(), samples.len(), lost.iter().filter(|s| !s.contains("日本語")).count(), lost.iter().filter(|s| !s.contains("日本語")).take(8).collect::<Vec<_>>());
}

fn random_documents(summary: &mut String, mechanism: Mechanism, name: &str, assets: &Path) {
    let page = Page::default();
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut ok_count = 0;
    let mut errors: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut panics: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for _ in 0..2_000 {
        let count = 1 + rng.below(30);
        let md: String = (0..count).map(|_| rng.pick(cases::DOC_TOKENS)).collect();
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let emitted = emit(&md, &page, mechanism);
            let result = compile(&EMBEDDED, emitted.source, Some(assets.to_path_buf()));
            result.document.and_then(|d| to_pdf(&d))
        }));
        match outcome {
            Ok(Ok(_)) => ok_count += 1,
            Ok(Err(e)) => {
                let key: String = e.chars().take(70).collect();
                errors.entry(key).or_insert((0, md.clone())).0 += 1;
            }
            Err(_) => {
                let at = PANIC_LOCATION.with(|l| l.borrow().clone());
                panics.entry(at).or_insert((0, md.clone())).0 += 1;
            }
        }
        typst::comemo::evict(1);
    }
    let _ = writeln!(summary, "\nrandom Markdown, 2,000 documents, {name}: {ok_count} compiled");
    for (error, (count, md)) in &errors {
        let _ = writeln!(summary, "  error x{count}: {error}\n    first input: {md:?}");
    }
    for (at, (count, md)) in &panics {
        let _ = writeln!(summary, "  panic x{count} at {at}\n    first input: {md:?}");
    }
}

// ---------------------------------------------------------------------------
// Child processes
// ---------------------------------------------------------------------------

fn deep_markdown(shape: &str, depth: usize) -> String {
    match shape {
        "quote" => "> ".repeat(depth) + "x\n",
        "list" => "- ".repeat(depth) + "x\n",
        "ordered" => "1. ".repeat(depth) + "x\n",
        "emphasis" => format!("{}x{}\n", "*a _b ".repeat(depth), " b_ a*".repeat(depth)),
        "link" => format!("{}x{}\n", "[".repeat(depth), "](/u)".repeat(depth)),
        other => panic!("unknown shape {other}"),
    }
}

fn child(args: &[String]) {
    match args[0].as_str() {
        "abort" => std::process::abort(),
        "deep" => {
            let md = deep_markdown(&args[1], args[2].parse().unwrap());
            let stack: usize = args[3].parse().unwrap();
            let work = move || {
                let emitted = emit(&md, &Page::default(), Mechanism::Table);
                println!("emitted {} bytes", emitted.source.len());
                let result = catch_unwind(AssertUnwindSafe(|| compile(&EMBEDDED, emitted.source, None)));
                match result {
                    Err(_) => println!("caught panic at {}", PANIC_LOCATION.with(|l| l.borrow().clone())),
                    Ok(r) => match r.document {
                        Err(e) => println!("compile error: {}", e.chars().take(90).collect::<String>()),
                        Ok(d) => {
                            println!("compiled {} pages", d.pages().len());
                            match to_pdf(&d) {
                                Ok(p) => println!("pdf {} bytes", p.len()),
                                Err(e) => println!("pdf error: {}", e.chars().take(90).collect::<String>()),
                            }
                        }
                    },
                }
            };
            if stack == 0 {
                work();
            } else {
                std::thread::Builder::new().stack_size(stack).spawn(work).unwrap().join().unwrap();
            }
        }
        "time" => {
            let md = if args[1] == "long" { long_document() } else { cases::STRUCTURE.replace("![Photo alt](photo.png)", "").replace("![Logo alt](logo.svg)", "") };
            let start = Instant::now();
            let emitted = emit(&md, &Page::default(), Mechanism::Table);
            let emit_time = start.elapsed();
            let start = Instant::now();
            let document = compile(&EMBEDDED, emitted.source, None).document.unwrap();
            let compile_time = start.elapsed();
            let start = Instant::now();
            let pdf = to_pdf(&document).unwrap();
            let pdf_time = start.elapsed();
            println!("{} pages, {} KB: emit {:.1}ms, compile {:.1}ms, pdf {:.1}ms, total {:.1}ms", document.pages().len(), pdf.len() / 1024, emit_time.as_secs_f64() * 1e3, compile_time.as_secs_f64() * 1e3, pdf_time.as_secs_f64() * 1e3, (emit_time + compile_time + pdf_time).as_secs_f64() * 1e3);
        }
        other => panic!("unknown child {other}"),
    }
}

fn run_deep(label: &str) {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = format!("label: {label}\nTimeout 30s per case, then a 1 GiB stack where the default stack aborted.\n\n");
    for shape in ["quote", "list", "ordered", "emphasis", "link"] {
        for depth in [10usize, 50, 100, 500, 1_000, 10_000] {
            let d = depth.to_string();
            let result = run_child(&["child", "deep", shape, &d, "0"], Duration::from_secs(30));
            let mut line = format!("{shape:>9} depth {depth:>6}: {result}");
            if result.starts_with("ABORT") {
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
    let _ = writeln!(out, "\ntiming, cold process, 5 runs each:");
    for shape in ["short", "long"] {
        for _ in 0..5 {
            let result = run_child(&["child", "time", shape], Duration::from_secs(120));
            let line = format!("{shape}: {result}");
            println!("{line}");
            let _ = writeln!(out, "{line}");
        }
    }
    let dir = manifest.join("results").join(label);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("deep.txt"), out).unwrap();
}

fn render_command(args: &[String]) {
    let mut page_file = None;
    let mut fonts_dir = None;
    let mut assets = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--page" => page_file = Some(args[i + 1].clone()),
            "--fonts" => fonts_dir = Some(PathBuf::from(&args[i + 1])),
            "--assets" => assets = Some(PathBuf::from(&args[i + 1])),
            other => panic!("unknown flag {other}"),
        }
        i += 2;
    }
    let fonts = store(fonts_dir.as_deref());
    let page = page_file.map_or(Page::default(), |f| parse_page(&std::fs::read_to_string(f).unwrap(), fonts.book()).unwrap());
    let markdown = std::fs::read_to_string(&args[0]).unwrap();
    let emitted = emit(&markdown, &page, if std::env::var("SPIKE_MECHANISM").as_deref() == Ok("literal") { Mechanism::Literal } else { Mechanism::Table });
    let compiled = compile(&fonts, emitted.source, assets);
    let document = compiled.document.unwrap();
    std::fs::write(&args[1], to_pdf(&document).unwrap()).unwrap();
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map_or(String::new(), |l| format!("{}:{}", l.file().rsplit('/').next().unwrap_or(""), l.line()));
        PANIC_LOCATION.with(|l| *l.borrow_mut() = location);
        eprintln!("{info}");
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => run_suites(&args[1]),
        Some("deep") => run_deep(&args[1]),
        Some("render") => render_command(&args[1..]),
        Some("dump") => {
            let dir = Path::new(&args[1]);
            std::fs::write(dir.join("structure.md"), cases::STRUCTURE).unwrap();
            std::fs::write(dir.join("long.md"), long_document()).unwrap();
            std::fs::write(dir.join("page.toml"), cases::PAGE_TOML).unwrap();
        }
        Some("repeat") => {
            let markdown = std::fs::read_to_string(&args[1]).unwrap();
            let assets = PathBuf::from(&args[2]);
            let mut distinct = HashSet::new();
            for i in 0..20 {
                if i % 5 == 4 {
                    typst::comemo::evict(0);
                }
                let emitted = emit(&markdown, &Page::default(), Mechanism::Table);
                let document = compile(&EMBEDDED, emitted.source, Some(assets.clone())).document.unwrap();
                distinct.insert(to_pdf(&document).unwrap());
            }
            println!("{} distinct result(s) in 20 runs in one process", distinct.len());
        }
        Some("child") => child(&args[1..]),
        _ => eprintln!("usage: spike-typst run|deep|render ..."),
    }
}
