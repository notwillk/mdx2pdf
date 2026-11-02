fn curated() -> Vec<(&'static str, String)> {
    let long = "x".repeat(200);
    let cases: Vec<(&str, String)> = vec![
        ("nested fences", "````\n```\ninner\n```\n````\n".into()),
        ("nested tilde in backtick fence", "```\n~~~\ninner\n~~~\n```\n".into()),
        ("fence with backticks in info-free body", "````\n``` not a close\n````\n".into()),
        ("fence with info string", "```rust title=\"a b\"\nfn main() {}\n```\n".into()),
        ("inline code containing backticks", "a `` b`c `` d\n".into()),
        ("inline code starting with a backtick", "`` `a ``\n".into()),
        ("escaped html tag", "\\<div>\n".into()),
        ("escaped html tag mid-text", "a \\<div> b\n".into()),
        ("literal asterisks", "2 \\* 3 \\* 4 and \\*not em\\*\n".into()),
        ("literal underscores", "snake_case_name and \\_not em\\_\n".into()),
        ("table cell with pipe", "| a |\n|---|\n| b \\| c |\n".into()),
        ("table cell with pipe in code", "| a |\n|---|\n| `b \\| c` |\n".into()),
        ("table with alignment", "| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n".into()),
        ("list in quote", "> - a\n> - b\n>   - c\n".into()),
        ("quote in list", "- > a\n  > b\n- c\n".into()),
        ("quote in list in quote", "> - > a\n>   >\n>   > b\n".into()),
        ("ordered list in quote with start", "> 7. a\n> 8. b\n".into()),
        ("footnote", "a[^1]\n\n[^1]: note\n".into()),
        ("footnote with blocks", "a[^n]\n\n[^n]: one\n\n    two\n\n    - three\n".into()),
        ("task list", "- [x] done\n- [ ] todo\n".into()),
        ("task list nested", "- [x] done\n  - [ ] sub\n".into()),
        ("strikethrough", "a ~~b~~ c\n".into()),
        ("strikethrough with emphasis", "~~a *b* c~~\n".into()),
        ("cjk next to emphasis", "これは**強調**です\n".into()),
        ("cjk punctuation next to strong", "**「強調」**です\n".into()),
        ("accented next to emphasis", "caf*é*s and *é*tude\n".into()),
        ("emoji next to emphasis", "🍩*glazed*🍩\n".into()),
        ("hard break", "a  \nb\\\nc\n".into()),
        ("loose and tight lists", "- a\n\n- b\n\n<!-- -->\n\n- c\n- d\n".into()),
        ("adjacent lists", "- a\n\n* b\n".into()),
        ("code after list", "- a\n\n```\nb\n```\n".into()),
        ("literal hash and number", "\\# no\n\n1\\. no\n\n\\- no\n".into()),
        ("link with title and parens", "[a](b(c) \"t\\\"u\")\n".into()),
        ("image with alt", "![a *b*](c.png \"t\")\n".into()),
        ("reference link", "[a][r]\n\n[r]: /u \"t\"\n".into()),
        ("long inline code, LF", format!("`{long}\n{long}`\n")),
        ("long inline code, CRLF", format!("`{long}\r\n{long}`\r\n")),
        ("long inline code, LF then break marker", format!("`{long}\n[y`\n")),
        ("long inline code, CRLF then break marker", format!("`{long}\r\n[y`\r\n")),
        ("literal autolink", "see https://a.b now\n".into()),
        ("autolink brackets", "<https://a.b>\n".into()),
        ("link whose text is its url", "[https://a.b](https://a.b)\n".into()),
        ("link whose text is another url", "[https://a.b](https://c.d)\n".into()),
        ("cjk beside literal asterisk", "これは\\*です\\*です\n".into()),
        ("cyrillic beside literal underscore", "при_вет_\n".into()),
        ("accented beside literal asterisk", "é\\*x\\*é\n".into()),
        ("CRLF document", "# a\r\n\r\n- b\r\n- c\r\n\r\n```\r\nd\r\n```\r\n".into()),
    ];
    cases
}

fn dialect_cases() -> Vec<(&'static str, &'static str)> {
    vec![
        ("escaped html tag", "\\<div>\n"),
        ("escaped html tag in quote", "> \\<div>\n"),
        ("escaped html tag in list", "- \\<div>\n"),
        ("escaped comment", "\\<!-- x -->\n"),
        ("escaped br", "a \\<br> b\n"),
        ("escaped autolink", "\\<https://a.b>\n"),
        ("four-space indented line", "    total\n"),
        ("indented after paragraph", "a\n\n    total\n"),
        ("indented two lines", "    total\n    more\n"),
        ("indented in quote", ">     total\n"),
        ("indented in list", "- a\n\n      total\n"),
        ("tab indented", "\ttotal\n"),
        ("escaped braces", "\\{curly\\} braces\n"),
        ("jsx in inline code", "`<A />`\n"),
        ("jsx in fence", "```\n<A />\n```\n"),
    ]
}

fn paragraph(value: &str) -> Node {
    Node::Paragraph(Paragraph {
        children: vec![text(value)],
        position: None,
    })
}

fn text(value: &str) -> Node {
    Node::Text(Text {
        value: value.into(),
        position: None,
    })
}

fn root(children: Vec<Node>) -> Node {
    Node::Root(Root {
        children,
        position: None,
    })
}

/// Trees no parser produced: what an Expression result looks like after Expansion.
fn hand_built() -> Vec<(String, Node)> {
    let values = [
        "A<B", "<div>", "<!-- x -->", "*x*", "**x**", "_x_", "~~x~~", "# x", "1. x", "1) x", "- x",
        "+ x", "> x", "`x`", "```", "~~~", "[a](b)", "![a](b)", "[a]: b", "[^1]", "a|b", "\\", "\\*",
        "&amp;", "---", "===", "https://a.b", "www.a.b", "a@b.c", "{x}", "{{ x }}", "$x$", "    x",
        " x", "x ", "a\nb", "a\n\nb", "a\n# b", "a  \nb", "é*x*é", "A_B_", "日本語_x_", "日本語*x*", "Ünï_cödé_",
    ];
    let mut cases = Vec::new();
    for value in values {
        cases.push((format!("paragraph: {value:?}"), root(vec![paragraph(value)])));
    }
    for value in ["*x*", "x #", "# x", "a\nb"] {
        cases.push((
            format!("heading: {value:?}"),
            root(vec![Node::Heading(Heading {
                children: vec![text(value)],
                position: None,
                depth: 2,
            })]),
        ));
    }
    for value in ["- x", "*x*", "1. x", "[ ] x", "[x] x"] {
        cases.push((
            format!("list item: {value:?}"),
            root(vec![Node::List(List {
                children: vec![Node::ListItem(ListItem {
                    children: vec![paragraph(value)],
                    position: None,
                    spread: false,
                    checked: None,
                })],
                position: None,
                ordered: false,
                start: None,
                spread: false,
            })]),
        ));
    }
    // PR #271 says the panic comes from long values with a CRLF. The slice it
    // names is taken before the CR check, so try a plain LF as well.
    let long = "x".repeat(200);
    for (name, value) in [
        ("short, LF", "a\nb".to_string()),
        ("short, CRLF", "a\r\nb".to_string()),
        ("long, LF", format!("{long}\n{long}")),
        ("long, CRLF", format!("{long}\r\n{long}")),
        ("long, LF then break marker", format!("{long}\n[y")),
        ("long, CRLF then break marker", format!("{long}\r\n[y")),
    ] {
        cases.push((
            format!("inline code value: {name}"),
            root(vec![Node::Paragraph(Paragraph {
                children: vec![Node::InlineCode(InlineCode {
                    value,
                    position: None,
                })],
                position: None,
            })]),
        ));
    }
    cases
}

