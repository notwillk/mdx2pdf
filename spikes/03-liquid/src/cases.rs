//! The table suite. Expected results were written before the first run. Those in
//! the `shopify` group come from Shopify's own pages, shopify.github.io/liquid
//! (basics/whitespace and tags/template), and from Shopify's documented behavior
//! for tabs. Don't edit them to match a result: a surprise is a finding.

pub enum Want {
    Out(&'static str),
    Err,
    /// An error whose message contains this name.
    Named(&'static str),
}
use Want::*;

const DATA: &str = "name: alice\nn: 3\nf: 1.5\nb: true\nz: null\nitems: [a, b]\nobj: {k: v}\nx: X\nrows:\n  - {name: one}\n  - {name: two}\n";

pub fn data() -> &'static str {
    DATA
}

/// `(group, template, wanted)`, all rendered with `DATA` as frontmatter.
pub const CASES: &[(&str, &str, Want)] = &[
    // Undefined values. Silent results here are predictions from the source, not approval.
    ("undefined", "{{ missing }}", Named("missing")),
    ("undefined", "{{ a.b.c }}", Named("a")),
    ("undefined", "{{ meta.missing }}", Named("missing")),
    ("undefined", "{{ obj.missing }}", Named("missing")),
    ("undefined", "{{ rows[5].name }}", Err),
    ("undefined", "{{ missing | default: \"x\" }}", Named("missing")),
    ("undefined", "{{ z | default: \"x\" }}", Out("x")),
    ("undefined", "[{{ z }}]", Out("[]")),
    ("undefined", "{% if missing %}y{% else %}n{% endif %}", Out("n")),
    ("undefined", "{% if obj.missing %}y{% else %}n{% endif %}", Out("n")),
    ("undefined", "{% unless missing %}y{% endunless %}", Out("y")),
    ("undefined", "{% if missing == 1 %}y{% else %}n{% endif %}", Named("missing")),
    ("undefined", "{% if missing and b %}y{% else %}n{% endif %}", Out("n")),
    ("undefined", "{% for i in missing %}y{% endfor %}", Named("missing")),
    ("undefined", "{% assign q = missing %}", Named("missing")),
    ("undefined", "{% case missing %}{% when 1 %}y{% endcase %}", Named("missing")),
    ("undefined", "{{ x | nosuchfilter }}", Named("nosuchfilter")),
    ("undefined", "{{ x | replace }}", Err),
    ("undefined", "{{ x | upcase: 1 }}", Err),
    ("undefined", "{% nosuchtag %}", Named("nosuchtag")),
    ("undefined", "{{ x", Err),
    ("undefined", "{% if x %}", Err),
    // Shopify's documented examples, exact.
    ("shopify", "{% assign my_variable = \"tomato\" %}\n{{ my_variable }}", Out("\ntomato")),
    ("shopify", "{% assign my_variable = \"tomato\" -%}\n{{ my_variable }}", Out("tomato")),
    (
        "shopify",
        "{% assign username = \"John G. Chalmers-Smith\" %}\n{% if username and username.size > 10 %}\n  Wow, {{ username }} , you have a long name!\n{% else %}\n  Hello there!\n{% endif %}",
        Out("\n\n  Wow, John G. Chalmers-Smith , you have a long name!\n"),
    ),
    (
        "shopify",
        "{% assign username = \"John G. Chalmers-Smith\" -%}\n{%- if username and username.size > 10 -%}\n  Wow, {{ username -}} , you have a long name!\n{%- else -%}\n  Hello there!\n{%- endif %}",
        Out("Wow, John G. Chalmers-Smith, you have a long name!"),
    ),
    (
        "shopify",
        "{% raw %}\nIn Handlebars, {{ this }} will be HTML-escaped, but {{{ that }}} will not.\n{% endraw %}",
        Out("\nIn Handlebars, {{ this }} will be HTML-escaped, but {{{ that }}} will not.\n"),
    ),
    // Shopify strips all whitespace at a hyphen, including tabs.
    ("shopify", "a\t{%- if true -%}\tb{% endif %}", Out("ab")),
    ("shopify", "a\t{{- x -}}\tb", Out("aXb")),
    ("shopify", "a\r\n{%- if true -%}\r\nb{% endif %}", Out("ab")),
    ("shopify", "a  \n  {%- if true -%}  \n  b{% endif %}", Out("ab")),
    ("shopify", "{{\tx\t}}", Out("X")),
    ("shopify", "{%\nif true\n%}y{%\nendif\n%}", Out("y")),
    ("shopify", "{{\nx\n}}", Out("X")),
    // Whitespace against quote prefixes: a trim also removes the line ending.
    ("whitespace", "> a\n{%- if b -%}\n> c\n{%- endif -%}\n> d\n", Out("> a> c> d\n")),
    ("whitespace", "> a\n{%- if b %}\n> c\n{%- endif %}\n> d\n", Out("> a\n> c\n> d\n")),
    ("whitespace", "> a\n{% if b %}\n> c\n{% endif %}\n> d\n", Out("> a\n\n> c\n\n> d\n")),
    // Raw.
    ("raw", "```\n{% raw %}${{ secrets.TOKEN }}{% endraw %}\n```\n", Out("```\n${{ secrets.TOKEN }}\n```\n")),
    ("raw", "{% raw %}{{ unclosed{% endraw %}", Out("{{ unclosed")),
    ("raw", "{% raw %}{% if %}{% endraw %}", Out("{% if %}")),
    ("raw", "{% raw %}{{ a }}{% endraw %} and {{ x }}", Out("{{ a }} and X")),
    ("raw", "{% raw %}", Err),
    ("raw", "{% endraw %}", Err),
    ("raw", "{% comment %}{{ missing }}{% endcomment %}", Out("")),
    // Liquid only treats its own delimiters as special.
    ("single brace", "{props.name | upcase}", Out("{props.name | upcase}")),
    ("single brace", "{ {{ x }} }", Out("{ X }")),
    ("single brace", "<A b={x} c=\"{{ x }}\" />", Out("<A b={x} c=\"X\" />")),
    ("single brace", "{% for i in items %}{i}{% endfor %}", Out("{i}{i}")),
    // Scope.
    ("scope", "{{ meta.x }}{{ x }}", Out("XX")),
    ("scope", "{% assign x = \"Y\" %}{{ x }}{{ meta.x }}", Out("YX")),
    ("scope", "{% assign meta = 1 %}{{ meta }}", Out("1")),
    ("scope", "{% for meta in items %}{{ meta }}{% endfor %}{{ meta.x }}", Out("abX")),
    ("scope", "{% include \"x\" %}", Err),
    ("scope", "{% include \"/etc/passwd\" %}", Err),
    ("scope", "{% render \"x\" %}", Named("render")),
];

/// Expressions evaluated to a typed value, shown as `type:value`.
pub const TYPED: &[(&str, &str)] = &[
    ("meta.items", "[string:a,string:b]"),
    ("items", "[string:a,string:b]"),
    ("meta.items | first", "string:a"),
    ("items | last", "string:b"),
    ("items | size", "whole number:2"),
    ("items | join: \",\"", "string:a,b"),
    ("rows", "[{name:string:one},{name:string:two}]"),
    ("rows[1].name", "string:two"),
    ("rows | first", "{name:string:one}"),
    ("obj", "{k:string:v}"),
    ("n", "whole number:3"),
    ("n | plus: 1", "whole number:4"),
    ("f", "fractional number:1.5"),
    ("b", "boolean:true"),
    ("z", "nil"),
    ("name", "string:alice"),
    ("name | upcase", "string:ALICE"),
    ("name | upcase | append: \"!\"", "string:ALICE!"),
    ("3", "whole number:3"),
    ("\"lit\"", "string:lit"),
    ("true", "boolean:true"),
    // In strings, delimiters are only text.
    ("\"}}\"", "string:}}"),
    ("\"%}\"", "string:%}"),
    ("\"{{\"", "string:{{"),
    ("\"{%\"", "string:{%"),
    ("name | append: \"}}\"", "string:alice}}"),
    ("name | replace: \"a\", \"}}\"", "string:}}lice"),
    ("name | replace: \"}\", \"\"", "string:alice"),
];

/// Text that tries to escape its wrapper. All should be rejected, one way or another.
pub const INJECTION: &[&str] = &[
    "name }} leak {{ name",
    "name }}{{ name",
    "\"\" }}{{ name",
    "\"\" }}{% assign name = \"changed\" %}{{ name",
    "name | upcase }}{{ name | upcase",
    "name %}",
    "{% if true %}",
    "{{ name }}",
    "name }}{% include \"x\" %}{{ name",
    "",
    "   ",
    "name name",
    "name |",
    "| upcase",
    // Added after the first run, once the output wrapper was seen to accept `"" }}{{ name`.
    // There's no expected result, because these were written after seeing behavior.
    "name %}{% assign __v = \"other\"",
    "\"\" %}{% assign __v = name",
    "'x' }}{{ name",
    "name ' }}{{ name '",
    "name \" }}{{ name \"",
    "\"a\" | append: \"}}\" }}{{ name",
];

pub const YAML: &[&str] = &[
    "a: {b: {c: [1, {d: e}]}}\n",
    "i: 3\n",
    "i: -3\n",
    "f: 1.0\n",
    "f: 1.50\n",
    "f: 1e3\n",
    "big: 9223372036854775807\n",
    "big: 99999999999999999999\n",
    "inf: .inf\n",
    "nan: .nan\n",
    "t: true\n",
    "t: yes\n",
    "t: on\n",
    "n: null\n",
    "n: ~\n",
    "n:\n",
    "d: 2026-01-02\n",
    "d: 2026-01-02T03:04:05Z\n",
    "s: \"quoted\"\n",
    "s: |\n  block\n  text\n",
    "s: 007\n",
    "base: &b {x: 1}\nref: *b\n",
    "base: &b {x: 1}\nm:\n  <<: *b\n  y: 2\n",
    "1: a\n",
    "true: a\n",
    "my-key: a\n",
    "a: 1\na: 2\n",
    "- not\n- a map\n",
    "just text\n",
    "",
    "# only a comment\n",
    "a: [1\n",
    "a: *missing\n",
];

/// Tried against every spelling, with `b` true and false. The count is block quotes.
pub const QUOTE_PATTERNS: &[(&str, &str)] = &[
    ("no trimming", "> a\n{% if c %}\n> b\n{% endif %}\n> d\n"),
    ("trim both sides", "> a\n{%- if c -%}\n> b\n{%- endif -%}\n> d\n"),
    ("trim left only", "> a\n{%- if c %}\n> b\n{%- endif %}\n> d\n"),
    ("trim right only", "> a\n{% if c -%}\n> b\n{% endif -%}\n> d\n"),
    ("same line", "> a{% if c %}\n> b{% endif %}\n> d\n"),
    ("prefix on tag lines", "> a\n> {% if c %}\n> b\n> {% endif %}\n> d\n"),
];

pub const PRD_INDEX: &str = "# {meta.title}\n\n{% for item in items %}\n<Callout kind=\"{{ item.name }}\">\nReview **{{ item.name }}** before {meta.title} ships.\n</Callout>\n{% endfor %}\n";
pub const PRD_YAML: &str = "title: Q3 Report\nitems:\n  - name: Alpha\n  - name: Beta\n";
pub const PRD_EXPECTED: &str = r#"Root[H1[ExprT("meta.title")],JsxF(Callout kind="Alpha")[P[T("Review "),Strong[T("Alpha")],T(" before "),ExprT("meta.title"),T(" ships.")]],JsxF(Callout kind="Beta")[P[T("Review "),Strong[T("Beta")],T(" before "),ExprT("meta.title"),T(" ships.")]]]"#;

pub const COMPONENT_BODY: &str = "> **{props.kind | upcase}**\n>\n> <Content />\n";
pub const COMPONENT_EXPECTED: &str = r#"Root[Q[P[Strong[ExprT("props.kind | upcase")]],JsxF(Content)]]"#;

/// Known hazards marked Spike 3: `(name, yaml, body)`. The messages get recorded.
pub const HAZARDS: &[(&str, &str, &str)] = &[
    ("secrets in a fence", "x: 1\n", "```yaml\nrun: echo ${{ secrets.TOKEN }}\n```\n"),
    ("company with a tag-like value", "company: \"A<B\"\n", "{{ company }}\n"),
    ("company with a tag-like value and more", "company: \"A<B, C\"\n", "{{ company }}\n"),
    ("quote in an attribute", "t: 'Say \"hi\"'\n", "<Card title=\"{{ t }}\" />\n"),
    ("loop variable in an Expression", "xs: [{name: a}]\n", "{% for i in xs %}<Card title={i.name} />{% endfor %}\n"),
    ("style object, the MDX habit", "x: 1\n", "<A style={{color: \"red\"}} />\n"),
    ("style object with a known name", "color: 1\n", "<A style={{color}} />\n"),
    ("assign then an Expression", "x: 1\n", "{% assign y = 2 %}{y}\n"),
    ("unknown tag", "x: 1\n", "{% includes 'a' %}\n"),
    ("percent in prose", "x: 1\n", "Growth of {% sales %} this year\n"),
];

/// Characters for the attribute round trip, as tokens.
pub const VALUE_TOKENS: &[&str] = &[
    "a", "b c", " ", "\"", "'", "&", "AT&T", "&amp;", "&#35;", "&b", "<", ">", "<A>", "</A>", "{", "}", "{x}", "{{", "}}", "{%", "%}",
    "\n", "\n\n", "\r\n", "\t", "\\", "é", "日本語", "🍩", "*", "_", "`", "[", "]", "|", "=", "/", "#", "-", "> ", "- ", "    ",
];

/// Tokens for random templates, including filters likely to divide by zero or overflow.
pub const TEMPLATE_TOKENS: &[&str] = &[
    "{{", "}}", "{%", "%}", "{{-", "-}}", "{%-", "-%}", "if x", "if", "else", "elsif n", "endif", "unless x", "endunless", "for i in xs", "for", "in", "endfor", "break", "continue", "raw", "endraw", "comment", "endcomment", "assign a = 1", "assign", "capture a", "endcapture", "case n", "when 1", "endcase", "cycle \"a\", \"b\"", "tablerow i in xs", "endtablerow", "increment k", "decrement k", "ifchanged", "endifchanged", "include", "x", "xs", "n", "a", "o", "o.k", "xs[0]", "xs[9]", "xs[-1]", "(1..3)", "(3..1)", "(1..n)", " ", "\n", "|", ":", ",", ".", "[", "]", "\"", "'", "\"a\"", "'b'", "é", "日本語", "1", "0", "-1", "1.5", "==", "!=", ">", "<", "contains", "and", "or", "nil", "empty", "blank", "true", "upcase", "first", "last", "size", "join", "sort", "uniq", "reverse", "map", "where",
];

/// Filter chains, so more templates reach filter code than random delimiters would.
pub const FILTERS: &[&str] = &[
    "upcase", "first", "last", "size", "sort", "uniq", "reverse", "compact", "abs", "ceil", "floor", "round", "round: 99", "round: -1", "strip", "newline_to_br", "escape", "url_encode", "url_decode", "capitalize", "slice: 1", "slice: -1", "slice: 99", "slice: 1, -5", "slice: 9223372036854775807", "truncate: 0", "truncate: 2", "truncate: -1", "truncate: 3, \"日本語\"", "truncatewords: 0", "truncatewords: -1", "split: \"\"", "split: \"é\"", "join: \"\"", "replace: \"\", \"x\"", "remove: \"\"", "replace_first: \"\", \"x\"", "prepend: \"a\"", "append: \"é\"", "plus: 1", "minus: 1", "times: 2", "divided_by: 0", "divided_by: 0.0", "modulo: 0", "modulo: 0.0", "times: 9223372036854775807", "plus: 9223372036854775807", "minus: 9223372036854775807", "divided_by: -1", "at_least: 1", "at_most: 1", "default: 1", "date: \"%Y\"", "date: \"%\"", "date: \"%Q\"", "map: \"k\"", "where: \"k\"", "concat: xs", "sum", "strip_html", "strip_newlines", "lstrip", "rstrip", "sort_natural", "escape_once", "last", "pop", "shift", "push: 1", "unshift: 1",
];
