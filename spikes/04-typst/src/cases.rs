//! Fixtures. Expected results were written before the first run. Don't edit them to
//! match a result: a surprise is a finding.

/// Text that must come out of the PDF unchanged. The spike's list, then what reading
/// Typst's syntax adds.
pub const ADVERSARIAL: &[&str] = &[
    "$5 and $6", "$x$", "Issue #42", "#", "@name", "<label>", "*", "*bold*", "_", "_em_", "`", "`raw`", "```", "= not a heading", "== nope", "\\", "\\n", "a\\b",
    "// not a comment", "/* not a comment */", "--", "---", "...", "~", "a~b", "-?", "\"straight\"", "'single'", "https://a.b", "http://a.b/c?d=e", "/ term: x", "+ x", "- x", "1. x", "]", "[", "[x]", "{", "}", "(", ")", ";", ":", ",", "%", "&", "|", "^", "#read(\"/etc/passwd\")", "#set page(width: 1cm)", "#pagebreak()", "#[x]", "#{1 + 1}", "#\"str\"", "\\u{41}", "<<>>", "=>", "->", "!=", "<=", "x_1^2", "a/b", "C:\\dir\\",
];

/// `(script, sample)`. Bundled fonts probably lack the last three.
pub const SCRIPTS: &[(&str, &str)] = &[
    ("latin accents", "Éléonore Ñandú Straße"),
    ("greek", "Καλημέρα κόσμε"),
    ("cyrillic", "Привет мир"),
    ("hebrew", "שלום עולם"),
    ("arabic", "مرحبا بالعالم"),
    ("cjk", "你好世界 こんにちは 안녕하세요"),
    ("devanagari", "नमस्ते दुनिया"),
    ("emoji", "🍩😀"),
    ("task glyphs", "☐☑"),
];

/// Every construct the emitter handles, with text that must appear.
pub const STRUCTURE: &str = r#"# Report Title

## Second level

### Third level

A paragraph with *emphasis*, **strong**, ~~struck~~, `inline code`, and a [link](https://example.com/a?b=c "title").
A soft break, and then a hard break:
after the break, with a backslash break:\
after that.

- bullet one
- bullet two
  - nested bullet
    - doubly nested
- bullet three

1. ordered one
2. ordered two
   1. nested ordered

7. started at seven
8. and eight

- loose one

- loose two

- [x] done task
- [ ] todo task
  - [x] nested task

| Left | Center | Right |
|:-----|:------:|------:|
| a1   | b1     | c1    |
| a2   | b2 \| pipe | c2 |
| short |

```rust
fn main() { println!("{}", "code # $ @ block"); }
```

```
no language
    indented * line
```

> A quote with **strong** text.
>
> > A nested quote.
>
> - a list in a quote

- A list holding a quote:

  > quoted in a list

A footnote here[^one], another[^two], and the first again[^one].

[^one]: The first note.

[^two]: The second note, with two paragraphs.

    Its second paragraph, with **strong**.

---

![Photo alt](photo.png)

![Logo alt](logo.svg)

Trailing paragraph.
"#;

pub const STRUCTURE_EXPECTED: &[&str] = &[
    "Report Title", "Second level", "Third level", "A paragraph with emphasis, strong, struck, inline code, and a link.", "after the break", "after that.", "bullet one", "nested bullet", "doubly nested", "ordered one", "nested ordered", "started at seven", "and eight", "loose one", "loose two", "done task", "todo task", "nested task", "Left", "Center", "Right", "a1", "b1", "c1", "b2 | pipe", "short", "code # $ @ block", "no language", "indented * line", "A quote with strong text.", "A nested quote.", "a list in a quote", "quoted in a list", "The first note.", "The second note, with two paragraphs.", "Its second paragraph, with strong.", "Trailing paragraph.",
];

/// `(name, markdown, should it compile)`, with the asset root at fixtures/assets.
pub const IMAGES: &[(&str, &str, bool)] = &[
    ("png", "![a](photo.png)\n", true),
    ("svg", "![a](logo.svg)\n", true),
    ("missing", "![a](nope.png)\n", false),
    ("parent directory", "![a](../secret.png)\n", false),
    ("deep parent directory", "![a](../../../../../../etc/hostname)\n", false),
    ("absolute path", "![a](/etc/hostname)\n", false),
    ("symlink leaving the root", "![a](escape-link.png)\n", false),
    ("url", "![a](http://127.0.0.1:9/x.png)\n", false),
    ("file url", "![a](file:///etc/hostname)\n", false),
    ("not an image", "![a](notes.txt)\n", false),
    // These two may compile with the reference left out. Either way, nothing outside the root may be read.
    ("svg referencing a file outside", "![a](outside.svg)\n", false),
    ("svg referencing an absolute file", "![a](abs.svg)\n", false),
    ("svg referencing a url", "![a](url.svg)\n", false),
];

/// `(name, page.toml, width, height in points, left margin in points, font in pdffonts, numbered)`.
pub const PAGES: &[(&str, &str, f64, f64, f64, &str, bool)] = &[
    ("default", "", 595.28, 841.89, 70.87, "LibertinusSerif", false),
    ("a4", "[paper]\nsize = \"a4\"\n", 595.28, 841.89, 70.87, "LibertinusSerif", false),
    ("letter", "[paper]\nsize = \"letter\"\n", 612.0, 792.0, 70.87, "LibertinusSerif", false),
    ("landscape", "[paper]\nsize = \"a4\"\norientation = \"landscape\"\n", 841.89, 595.28, 70.87, "LibertinusSerif", false),
    ("custom size", "[paper]\nwidth = \"100mm\"\nheight = \"200mm\"\n", 283.46, 566.93, 70.87, "LibertinusSerif", false),
    ("margins", "[margins]\ntop = \"10mm\"\nright = \"30mm\"\nbottom = \"40mm\"\nleft = \"50mm\"\n", 595.28, 841.89, 141.73, "LibertinusSerif", false),
    ("font", "[type]\nfont = \"New Computer Modern\"\n", 595.28, 841.89, 70.87, "NewCM", false),
    ("size", "[type]\nsize = \"20pt\"\n", 595.28, 841.89, 70.87, "LibertinusSerif", false),
    ("page numbers", "[page_numbers]\nenabled = true\n", 595.28, 841.89, 70.87, "LibertinusSerif", true),
];

/// Must all be errors from our own validation, before Typst sees them.
pub const BAD_PAGES: &[&str] = &[
    "[paper]\nsize = \"a5x\"\n",
    "[paper]\nnope = \"x\"\n",
    "[nope]\nx = 1\n",
    "[type]\nfont = \"No Such Font\"\n",
    "[type]\nsize = \"11pt\"); #read(\"x\") //\"\n",
    "[margins]\nleft = \"1cm)\"\n",
    "[type]\nfont = \"x\\\"), #pagebreak()\"\n",
];

/// Spike 1b's document tokens, for documents that must always compile.
pub const DOC_TOKENS: &[&str] = &[
    "a", "b c", "word ", " ", "*", "**", "_", "__", "~~", "`", "``", "[", "]", "](/u)", "](/u \"t\")",
    "![", "[r]", "[r][]", "[^1]", "<", ">", "&amp;", "&#35;", "\\", "\\*", "é", "日本語", "при", "🍩",
    "\n", "\n\n", "\n\n> ", "\n> ", "\n- ", "\n  - ", "\n* ", "\n1. ", "\n2) ",
    "\n- [x] ", "\n- [ ] ", "\n# ", "\n## ", "\n---\n", "\n===\n", "\n```\n", "\n```rust\n", "\n~~~\n",
    "\n    ", "\n| a | b |\n|---|:-:|\n| ", "  \n", "\\\n", "\n[r]: /v \"t\"\n", "\n[^1]: note\n", "|", "<div>", "<!-- c -->", "$", "#", "@", "=", "//",
];

/// Tokens for the escaping fuzz.
pub const FUZZ_TOKENS: &[&str] = &[
    "a", "bc", " ", "  ", "\t", "$", "#", "@", "<", ">", "*", "_", "`", "=", "\\", "/", "//", "/*", "*/", "-", "--", "---", "...", ".", "~", "?", "-?", "\"", "'", "[", "]", "{", "}", "(", ")", ";", ":", ",", "%", "&", "|", "^", "+", "!", "http://x", "https://a.b", "#set", "#let x = 1", "#read(\"/etc/passwd\")", "\\u{41}", "1.", "é", "日本語", "Привет", "ﬁ", "fi", "\u{a0}", "\u{ad}", "\u{200b}", "\u{2028}", "🍩",
];

/// Written to disk for check-determinism.sh, beside fixtures/assets.
pub const PAGE_TOML: &str = "[paper]\nsize = \"letter\"\norientation = \"landscape\"\n\n[margins]\nleft = \"30mm\"\n\n[type]\nsize = \"12pt\"\n\n[page_numbers]\nenabled = true\n";

/// A real start number needs a separate list.
pub const STARTED_LIST: &str = "Before the list.\n\n7. started at seven\n8. and eight\n";
