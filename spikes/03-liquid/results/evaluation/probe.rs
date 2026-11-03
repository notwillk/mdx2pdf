// Review probes for Spike 3, added to a scratch copy only. Liquid, then the Dialect, then Expressions.

fn tree(node: &Node, depth: usize, out: &mut String) {
    let name = match node {
        Node::Text(n) => format!("Text({:?})", n.value),
        Node::InlineCode(n) => format!("InlineCode({:?})", n.value),
        Node::Code(n) => format!("Code({:?})", n.value),
        Node::MdxFlowExpression(n) => format!("ExprFlow({:?})", n.value),
        Node::MdxTextExpression(n) => format!("ExprText({:?})", n.value),
        Node::MdxJsxFlowElement(n) => format!("JsxFlow({}{})", n.name.clone().unwrap_or_default(), attributes(&n.attributes)),
        Node::MdxJsxTextElement(n) => format!("JsxText({}{})", n.name.clone().unwrap_or_default(), attributes(&n.attributes)),
        other => format!("{other:?}").split([' ', '(']).next().unwrap_or("?").to_string(),
    };
    let _ = writeln!(out, "{}{}", "  ".repeat(depth), name);
    if let Some(children) = node.children() {
        for child in children {
            tree(child, depth + 1, out);
        }
    }
}

fn pipeline_case(yaml: &str, body: &str) {
    let parser = parser();
    let globals = scope(yaml);
    println!("=== yaml {yaml:?} body {body:?}");
    match render(&parser, body, &globals) {
        Rendered::Output(out) => {
            println!("liquid output: {out:?}");
            match parse_dialect(&out) {
                Parsed::Tree(t) => {
                    let mut text = String::new();
                    tree(&t, 1, &mut text);
                    print!("{text}");
                    let mut exprs = Vec::new();
                    collect_expressions(&t, &mut exprs);
                    for e in exprs {
                        let result = if has_delimiter_outside_string(&e) {
                            "rejected by the delimiter scan".to_string()
                        } else {
                            eval_output(&parser, &e, &globals).map_or_else(|m| format!("ERR {}", m.chars().take(150).collect::<String>()), |v| show(&v))
                        };
                        println!("  expression {e:?}: {result}");
                    }
                }
                other => println!("  {}", describe_parsed(&other)),
            }
        }
        other => println!("liquid: {}", describe(&other)),
    }
}

fn pipeline_suite() {
    let cases: &[(&str, &str)] = &[
        // What {% raw %} produces reaches the Dialect.
        ("x: 1\n", "A {% raw %}{{ x }}{% endraw %} B\n"),
        ("x: 1\n", "{% raw %}{{ x }}{% endraw %}\n"),
        ("x: 1\n", "{% raw %}{% if y %}{% endraw %}\n"),
        ("x: 1\n", "{% raw %}{x}{% endraw %}\n"),
        ("x: 1\n", "<A>{% raw %}{{ x }}{% endraw %}</A>\n"),
        ("x: 1\n", "<A>\n{% raw %}{{ x }}{% endraw %}\n</A>\n"),
        ("x: 1\n", "```\n{% raw %}{{ x }}{% endraw %}\n```\n"),
        ("x: 1\n", "`{% raw %}{{ x }}{% endraw %}`\n"),
        ("x: 1\n", "{% raw %}\\{\\{ x }}{% endraw %}\n"),
        ("x: 1\n", "{% raw %}\\{% if y %}{% endraw %}\n"),
        // A Liquid value is read as MDX, so braces in data become Expressions.
        ("x: \"{y}\"\ny: secret\n", "{{ x }}\n"),
        ("x: \"Save 50% {now\"\n", "{{ x }}\n"),
        ("x: \"{{ y }}\"\ny: secret\n", "{{ x }}\n"),
        // Clock through comparison.
        ("d: 2026-01-02\n", "{% if d < \"now\" %}before{% else %}not{% endif %}\n"),
        // Attributes in places the spike didn't place them.
        ("t: \"a|b\"\n", "| h |\n|---|\n| <Card title=\"{{ t | attr }}\" /> |\n"),
        ("t: \"a|b\"\n", "| <Card title=\"{{ t | attr }}\" /> |\n|---|\n| b |\n"),
        ("t: \"it's\"\n", "<Card title='{{ t | attr }}' />\n"),
        // Tabs in an Expression.
        ("x: a\n", "{x\t| upcase}\n"),
    ];
    for (yaml, body) in cases {
        pipeline_case(yaml, body);
    }
}

fn attr_fixed(value: &str) -> String {
    attr_escape(value).replace('|', "&#124;").replace('\'', "&#39;")
}

fn attr_placements() {
    let placements: &[(&str, &str)] = &[
        ("double quotes (control)", "<Card title=\"V\" />\n"),
        ("table body", "| h |\n|---|\n| <Card title=\"V\" /> |\n"),
        ("table header", "| <Card title=\"V\" /> |\n|---|\n| b |\n"),
        ("list item", "- <Card title=\"V\" />\n"),
        ("nested tag", "<A>\n<Card title=\"V\" />\n</A>\n"),
        ("single quotes", "<Card title='V' />\n"),
    ];
    for (name, escape) in [("attr as specified", attr_escape as fn(&str) -> String), ("attr plus | and '", attr_fixed as fn(&str) -> String)] {
        println!("--- {name}");
        for (placement, template) in placements {
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
            let mut totals: BTreeMap<&str, usize> = BTreeMap::new();
            let mut first: BTreeMap<&str, String> = BTreeMap::new();
            for _ in 0..20_000 {
                let count = 1 + rng.below(6);
                let value: String = (0..count).map(|_| rng.pick(cases::VALUE_TOKENS)).collect();
                let text = template.replace('V', &escape(&value));
                let outcome = match parse_dialect(&text) {
                    Parsed::Tree(tree) => match find_attribute(&tree) {
                        Some(found) if found == value => "same",
                        Some(_) => "different value",
                        None => "no attribute",
                    },
                    Parsed::Error { .. } => "dialect error",
                    Parsed::Panic(_) => "dialect panic",
                };
                *totals.entry(outcome).or_default() += 1;
                if outcome != "same" {
                    first.entry(outcome).or_insert(value);
                }
            }
            println!("  {placement}: {totals:?}  first misses: {first:?}");
        }
    }
}

fn object_print() {
    let globals = scope("obj: {zebra: 1, apple: 2, mango: 3, kiwi: 4, fig: 5, lime: 6}\n");
    println!("{}", describe(&render(&parser(), "{{ obj }}", &globals)));
}

fn has_table(node: &Node) -> bool {
    matches!(node, Node::Table(_)) || node.children().is_some_and(|c| c.iter().any(has_table))
}

fn table_loss() {
    for (name, escape) in [("attr as specified", attr_escape as fn(&str) -> String), ("attr plus | and '", attr_fixed as fn(&str) -> String)] {
        for (placement, template) in [("table header", "| <Card title=\"V\" /> |\n|---|\n| b |\n"), ("table body", "| h |\n|---|\n| <Card title=\"V\" /> |\n")] {
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
            let (mut kept, mut lost, mut errors) = (0, 0, 0);
            for _ in 0..20_000 {
                let count = 1 + rng.below(6);
                let value: String = (0..count).map(|_| rng.pick(cases::VALUE_TOKENS)).collect();
                match parse_dialect(&template.replace('V', &escape(&value))) {
                    Parsed::Tree(tree) if has_table(&tree) => kept += 1,
                    Parsed::Tree(_) => lost += 1,
                    _ => errors += 1,
                }
            }
            println!("{name}, {placement}: table kept {kept}, silently not a table {lost}, errors {errors}");
        }
    }
}

fn pipeline_more() {
    let cases: &[(&str, &str)] = &[
        // Literal braces in prose with no raw at all.
        ("x: 1\n", "A \\{\\{ x }} B\n"),
        ("x: 1\n", "A \\{\\% if y %} B\n"),
        ("x: 1\n", "<A>\\{\\{ x }}</A>\n"),
        ("x: 1\n", "<A>\\{x}</A>\n"),
        ("x: 1\n", "| h |\n|---|\n| \\{\\{ x }} |\n"),
        // Multi-line Liquid values inside containers.
        ("x: \"a\\n\\nb\"\n", "> {{ x }}\n"),
        ("x: \"a\\nb\"\n", "> {{ x }}\n"),
        ("x: \"a\\n- b\"\n", "- {{ x }}\n"),
        ("x: \"a\\nb\"\n", "| h |\n|---|\n| {{ x }} |\n"),
        // The same values as Expressions, for comparison.
        ("x: \"a\\n\\nb\"\n", "> {x}\n"),
    ];
    for (yaml, body) in cases {
        pipeline_case(yaml, body);
    }
}

/// What a `text` filter could write: the value, safe to print in MDX source.
fn text_escape(value: &str) -> String {
    let mut out = String::new();
    for c in value.trim().chars() {
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

fn all_text(node: &Node, out: &mut String) {
    match node {
        Node::Text(n) => out.push_str(&n.value),
        other => {
            if let Some(children) = other.children() {
                children.iter().for_each(|c| all_text(c, out));
            }
        }
    }
}

fn kinds(node: &Node, out: &mut String) {
    let name = format!("{node:?}");
    if !matches!(node, Node::Text(_)) {
        out.push_str(name.split([' ', '(']).next().unwrap_or("?"));
        out.push(',');
    }
    if let Some(children) = node.children() {
        children.iter().for_each(|c| kinds(c, out));
    }
}

fn text_placements() {
    let placements: &[(&str, &str)] = &[
        ("paragraph", "A V B\n"),
        ("heading", "# A V B\n"),
        ("list item", "- A V B\n"),
        ("quote", "> A V B\n"),
        ("table cell", "| h |\n|---|\n| A V B |\n"),
        ("strong", "**A V B**\n"),
        ("tag child", "<A>A V B</A>\n"),
    ];
    for (name, escape) in [("plain {{ t }}", (|v: &str| v.trim().to_string()) as fn(&str) -> String), ("{{ t | text }}", text_escape as fn(&str) -> String)] {
        println!("--- {name}");
        for (placement, template) in placements {
            // The shape with a harmless value, so added or lost structure counts as a difference.
            let base = match parse_dialect(&template.replace('V', "plain")) {
                Parsed::Tree(t) => {
                    let mut k = String::new();
                    kinds(&t, &mut k);
                    k
                }
                _ => panic!("control"),
            };
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
            let mut totals: BTreeMap<&str, usize> = BTreeMap::new();
            let mut first: BTreeMap<&str, String> = BTreeMap::new();
            for _ in 0..20_000 {
                let count = 1 + rng.below(6);
                let value: String = (0..count).map(|_| rng.pick(cases::VALUE_TOKENS)).collect();
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let outcome = match parse_dialect(&template.replacen('V', &escape(&value), 1)) {
                    Parsed::Tree(t) => {
                        let (mut text, mut k) = (String::new(), String::new());
                        all_text(&t, &mut text);
                        kinds(&t, &mut k);
                        if k != base {
                            "structure changed"
                        } else if text == format!("A {trimmed} B") || text == format!("hA {trimmed} B") {
                            "same"
                        } else {
                            "text changed"
                        }
                    }
                    Parsed::Error { .. } => "error",
                    Parsed::Panic(_) => "panic",
                };
                *totals.entry(outcome).or_default() += 1;
                if outcome != "same" {
                    first.entry(outcome).or_insert(value);
                }
            }
            println!("  {placement}: {totals:?}  first: {first:?}");
        }
    }
}
