//! Spike 1b: shrinks the inputs that panicked the markdown parser, for Spike 2 and upstream.

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};

use markdown::{to_mdast, Constructs, ParseOptions};
use pulldown_cmark::{Options as Flags, Parser};

thread_local! {
    static LOCATION: RefCell<String> = const { RefCell::new(String::new()) };
}

fn gfm_no_literal() -> ParseOptions {
    ParseOptions {
        constructs: Constructs {
            gfm_autolink_literal: false,
            ..Constructs::gfm()
        },
        ..ParseOptions::gfm()
    }
}

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

fn panics_at(input: &str, options: &ParseOptions) -> Option<String> {
    catch_unwind(AssertUnwindSafe(|| {
        let _ = to_mdast(input, options);
    }))
    .err()
    .map(|_| LOCATION.with(|l| l.borrow().clone()))
}

/// Removes characters while the same line keeps panicking.
fn reduce(input: &str, location: &str, options: &ParseOptions) -> String {
    let mut chars: Vec<char> = input.chars().collect();
    let mut size = chars.len().max(2) / 2;
    while size >= 1 {
        let mut start = 0;
        while start + size <= chars.len() {
            let mut candidate = chars.clone();
            candidate.drain(start..start + size);
            let text: String = candidate.iter().collect();
            if panics_at(&text, options).as_deref() == Some(location) {
                chars = candidate;
            } else {
                start += 1;
            }
        }
        size /= 2;
    }
    chars.into_iter().collect()
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map_or(String::new(), |l| format!("{}:{}", l.file().rsplit('/').next().unwrap_or(""), l.line()));
        LOCATION.with(|l| *l.borrow_mut() = location);
    }));
    let inputs = [
        "\n2) ```\n  - >~~|",
        "\\*\n- \n===\n日本語``>\n- ",
        "<>[r][]\n---\n_**a\n---\n\n## _\n| a | b |\n|---|:-:|\n| \n* \n\n\n]",
    ];
    let flags = Flags::ENABLE_TABLES | Flags::ENABLE_FOOTNOTES | Flags::ENABLE_STRIKETHROUGH | Flags::ENABLE_TASKLISTS;
    for input in inputs {
        let (name, options, location) = match panics_at(input, &gfm_no_literal()) {
            Some(location) => ("gfm", gfm_no_literal(), location),
            None => {
                let options = dialect();
                let location = panics_at(input, &options).expect("this input panics");
                ("dialect", options, location)
            }
        };
        let small = reduce(input, &location, &options);
        let stock_gfm = panics_at(&small, &ParseOptions::gfm());
        let commonmark = panics_at(&small, &ParseOptions::default());
        let pulldown = catch_unwind(|| Parser::new_ext(&small, flags).count()).is_err();
        println!(
            "{location} (found with {name})\n  minimal: {small:?}\n  stock gfm options panic: {stock_gfm:?}\n  commonmark options panic: {commonmark:?}\n  pulldown-cmark panics: {pulldown}"
        );
    }

    // How ordinary is the first shape? An unclosed fence is a common typo.
    println!("\nprobes, gfm options:");
    for probe in [
        "1. ```\n- x\n",
        "- ```\n1. x\n",
        "- ```\n* x\n",
        "1. ```\n2) x\n",
        "- ```\n  code\n* x\n",
        "- ```\n  code\n  ```\n* x\n",
        "1. ```\n   code\n   ```\n- x\n",
        "- ~~~\n1. x\n",
        "> - ```\n> 1. x\n",
        "```\n- x\n",
        "- ```\n",
    ] {
        println!("  {probe:?}: panic at {:?}", panics_at(probe, &ParseOptions::gfm()));
    }
}
