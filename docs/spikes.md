# Spikes

Short experiments to test the riskiest assumptions in the [technical design](technical-design.md) before building on them. Status as of 2026-11-02: Spike 1 failed, and Spikes 2–5 haven't started.

## Ground rules

- **Throwaway code.** Each spike lives in `spikes/NN-name/` and gets deleted once its findings are recorded. Nothing graduates into the real crates without being rewritten.
- **Timeboxed.** When time runs out, record what's known and decide whether more is worth it.
- **Objective result.** Each spike has a pass condition set in advance. Change a bar only by writing down why.
- **Findings go here.** A result that settles a hard-to-reverse decision also gets an [ADR](adr/), and the design gets updated.

**Prerequisite:** Rust 1.92 or later, which the Typst crates need. `.devcontainer/install.sh` installs it each time the container starts, if it's missing.

## Overview

| # | Question | Timebox | If it fails |
|---|---|---|---|
| [1](#spike-1-markdown-serializer) | Can `mdast_util_to_markdown` round-trip reliably? | 0.5 day | Write our own serializer |
| [2](#spike-2-dialect-parsing) | Does `markdown` parse the Dialect? | 0.5 day | Work around the gaps, or change parser |
| [3](#spike-3-liquid-behavior) | Does `liquid` behave the way the design needs? | 1 day | Custom filters, weaker errors, or a fork |
| [4](#spike-4-typst-embedding) | Can Typst embed cleanly and deterministically? | 1–2 days | Try Fulgur |
| [5](#spike-5-end-to-end-expansion) | Does Expansion work end to end? | 1–2 days | Revisit the expansion design |

Spikes 1–4 are independent and can run in parallel. Spike 5 needs 1–3. If running one at a time, start with Spike 1, because it's the cheapest way to test the biggest risk.

## Spike 1: Markdown serializer

**Question:** Can `mdast_util_to_markdown` turn our trees into Markdown without changing their meaning?

**Why it matters:** The Intermediate Markdown depends on this step, and a bug here would silently change documents. This crate is the weakest link in the plan: version 0.0.2, with a known panic addressed by an unmerged PR.

**Method:** Parse, serialize, and parse again with `markdown`, then compare the two trees with positions removed.

**Inputs:**

- All CommonMark spec examples.
- About 30 curated cases: nested fences, fences containing backticks, `\<div>`, literal `*` and `_`, table cells containing pipes, lists in quotes, quotes in lists, footnotes, task lists, strikethrough, non-ASCII text next to emphasis, and long inline code with CRLF line endings.

**Also find out:**

- Should we use the crates.io release or a git dependency? Its crates.io entry has no repository link, though the same maintainer published it from the `markdown-rs` repo.
- Do its tree types match `markdown` 1.0.0?

**Pass:** Equal trees on all curated cases and at least 99% of spec examples, with no panics. Each miss is listed with an explanation.

**If it fails:** Write our own serializer, covering only the node kinds we produce. The spike should end with an estimate of that work.

**Findings (2026-11-02): failed.** Write our own serializer, which should take about 2–3 days. Harness and raw output are in `spikes/01-serializer/`, and `results/record/misses.txt` has every miss with its input and output.

| Bar | Result |
|---|---|
| At least 99% of spec examples (at most 6 misses) | **93.40%:** 609 of 652, 43 misses |
| All curated cases equal | **33 of 47:** 14 misses |
| No panics | **4 inputs panic**, from 2 separate bugs |

The curated list grew from 38 to 47 cases during the spike, once the first results pointed at new failures. The original 38 had 9 misses, and the 9 added cases had 5, so the original list fails without them.

**Curated misses (14):**

| Cases | Result | Cause |
|---|---|---|
| Tables (3), footnotes (2), strikethrough (2) | `Err` | No handler for these node kinds |
| Task lists (2) | **Silent:** checkboxes dropped, so `- [x] done` becomes `* done` | The list item handler ignores `checked` |
| Literal `*` beside non-ASCII text (2) | **Silent:** text changes, so `é\*x\*é` becomes `éxxéé`, and `これは\*です\*です` gains a line break | See bug 1 |
| Literal `_` beside Cyrillic (1) | Panic | See bug 1 |
| Long inline code with a line break before `[` (2) | Panic | See bug 2 |

**Spec misses (43):**

| Examples | Count | Reaches Stage 5? | Effect |
|---|---|---|---|
| 148, 168, 187, 201, 475–477, 491, 494, 524, 536, 613–615, 617, 623, 625–631 | 23 | **No** | Whitespace added or changed around inline `Html` nodes. Rendered output changes, but `Html` nodes can't reach Stage 5: none appeared in 624 Dialect parses, and Markdown Input joins at Stage 6 |
| 554, 558, 559, 573, 576, 585, 589 | 7 | Yes | Emphasis in a reference label is lost, so the link or image becomes plain text |
| 354, 375, 388, 401 | 4 | Yes | Non-ASCII text silently corrupted (bug 1) |
| 20, 603 | 2 | Yes | A backslash in an autolink is doubled, so the URL changes. `resource_link: true` fixes these two but breaks others (91.72% on the GFM spec run) |
| 461, 463 | 2 | Yes | `*_foo_*` becomes `**foo**`, so nested emphasis turns into strong |
| 39 | 1 | Yes | A blank line in a text value splits the paragraph in two |
| 362 | 1 | Yes | Panic (bug 1) |
| 194, 564 | 2 | Yes | Harmless: shortcut reference becomes full, same rendered HTML |
| 306 | 1 | Yes | Harmless: `spread` differs on one list item, same rendered HTML |

Ignoring the 23 that can't reach Stage 5 still leaves 20 misses, over the limit of 6. "Harmless" means the crate's own HTML output is identical for the input and the serialized text. Only 3 of the 43 meet that.

**The two panic bugs:**

1. **Non-ASCII text (`state.rs:513`).** `value.chars().nth(*position)` uses a byte offset as a character index. After any non-ASCII text, it reads the wrong character and encodes that instead, or slices inside a character. This is the more serious bug, and no open PR we looked at addresses it.
2. **Inline code (`inline_code.rs:54`).** This is the PR #271 panic, but **it isn't specific to CRLF**. `&value[position..m.len()]` is evaluated before the CR check, so plain LF panics too, on input straight from the parser: a long code span with a line break before `[`. Normalizing to LF doesn't avoid it, which corrects the earlier claim in the design. The same text in `inline_math.rs` has the same bug, though the Dialect doesn't use math.

**Beyond the spike's method:**

- **Dialect → Markdown (14 of 15 equal).** The `\<div>` and `    total` hazards both pass. The miss is `\<https://a.b>`: the literal autolink takes the `>`, and re-parsing nests a link inside a link. `[https://a.b](https://c.d)` also parses to a nested link, which is a `markdown` quirk for Spike 2 to check.
- **Hand-built trees, like Expression results (49 of 59 equal).** Most ASCII values escape correctly, including `A<B`, `*x*`, `# x`, `[a](b)`, and `[x] x`. The misses show what our serializer has to get right:
  - URLs and emails (`https://a.b`, `www.a.b`, `a@b.c`) become links.
  - `a\n\nb` splits a paragraph.
  - A heading with the text `# x` loses its `#`.
  - Non-ASCII values hit bug 1 (3 cases), and inline code values hit bug 2 (2 cases).

**Configuration and unmerged fixes don't rescue it:**

| Run | Spec | Curated | Notes |
|---|---|---|---|
| Crates.io source, default options | 93.40% | 33/47 | Run of record |
| `fences: true` | 93.40% | 33/47 | No change |
| `resource_link: true` | 93.71% | 29/47 | Worse overall |
| PR #184 (tables) | 93.40% | 35/47 | Only tables. A pipe in a code span is left unescaped, which breaks the cell |
| PR #271 (panic) | 93.40% | 33/47 | Bug 2 becomes a silent newline-to-space change. Bug 1 still panics |
| Both PRs plus a one-line fix for bug 1 | 94.17% | 38/47 | No panics, but still fails |

**Also found out:**

- **Crates.io or git:** neither, since we won't use the crate. For the record, the published 0.0.2 tarball is byte-identical to upstream's tagged commit `fb7fdec` apart from Cargo's manifest changes, and `markdown` 1.0.0's `src/` matches that commit too. So its missing repository link isn't a provenance problem. Because upstream documents no registry install for this crate, the harness used upstream git at `1506572` rather than the crates.io package. The code is the same.
- **Tree types:** they match. Published 0.0.2 requires `^1.0.0-alpha.24`, which resolves to `markdown` 1.0.0 with a single copy in the graph, and the harness compiles against it.

**Estimate: about 2–3 days for our own serializer.**

- **Scope:** the 24 node kinds Stage 5 can receive: `Root`, `Paragraph`, `Heading`, `ThematicBreak`, `Blockquote`, `List`, `ListItem`, `Code`, `Definition`, `Text`, `Emphasis`, `Strong`, `Delete`, `InlineCode`, `Break`, `Link`, `LinkReference`, `Image`, `ImageReference`, `Table`, `TableRow`, `TableCell`, `FootnoteDefinition`, and `FootnoteReference`. There's no `Html`, math, or MDX.
- **Size:** roughly 600–900 lines, about 1.5–2 days. Most of the risk is in escaping and in choosing delimiters for nested emphasis and code. These figures are judgment, not measurement.
- **Tests:** about 0.5–1 day. These suites and misses convert directly into the required round-trip tests.
- **Why not fork:** a fork keeps about 3,100 lines with no active upstream. It would still need task lists, footnotes, strikethrough, a table fix, and reference and emphasis fixes, which is comparable work on top of code with known byte/character confusion. It also escapes using a pattern table, where ours can escape conservatively because it only needs to satisfy one parser.
- **Constraint:** PRD success criterion 1 pins the exact output for the PRD's example, so output has to stay readable rather than escaping all punctuation.

**Not tested:** the harness can't catch a stack overflow, and none occurred. It didn't fuzz inputs beyond these suites.

## Spike 2: Dialect parsing

**Question:** With JSX and expressions on and ESM off, does `markdown` give us the tree the design needs?

**Why it matters:** The docs say each construct has its own flag, but don't say whether this combination works. And `Constructs::mdx()` also turns off autolinks, indented code, and HTML, which makes the Dialect differ from Markdown.

**Method:** Build a custom `Constructs` with `mdx_jsx_flow`, `mdx_jsx_text`, `mdx_expression_flow`, `mdx_expression_text`, and the GFM constructs on, and `mdx_esm`, `html_flow`, `html_text`, `code_indented`, and `autolink` off. Print the tree for each input below.

| Input | Looking for |
|---|---|
| `<A>` inside lists, quotes, and other `<A>` tags | Correct nesting |
| `<Note>hi</Note>` alone on a line | Inline in a paragraph, as the design assumes |
| `{props.name \| upcase}` in text and on its own line | Expression text preserved exactly |
| `<A b={1} c="x" d />`, `<A {...b} />` | How each attribute kind appears |
| `{x \| replace: "}", ""}` | How brace matching treats strings |
| `Use {curly} braces`, an unmatched `{` | Expression or error, and which |
| `{/* comment */}` | Usable as a comment |
| `<br>`, `<div>`, `<!-- x -->`, `<https://a.b>` | Error message and location |
| `import Chart from './Chart.mdx'` | Prints as text, so how do we detect it? Compare a line check against passing a rejecting `mdx_esm_parse` |
| `` `<A />` ``, a fence containing `<A />` | Stays code |
| Random and malformed input | Returns `Err`, never panics |

**Also find out:** Is the crate's lack of commits since 2025-04-23 a practical problem? Check whether the open PRs include bugs we'd hit, and how hard a fork would be.

**Pass:** Correct nesting, Expression text preserved, no panics, and a written list of how the Dialect differs from Markdown.

**If it fails:** Pre-process around small gaps, fork if a fix is needed, or write a small tag scanner of our own, as executable.md does.

**Findings:** _Not started._

## Spike 3: Liquid behavior

**Question:** Does `liquid` do what both the Liquid Pass and Expressions need?

**Why it matters:** The chosen syntax depends on it ([ADR 0001](adr/0001-liquid-is-the-only-logic-language.md)), and its last release was in February 2025.

**Method:** Run about 20 templates through a small harness that parses YAML frontmatter with `serde-saphyr`.

| Area | Test |
|---|---|
| Undefined values | `{{ missing }}` and `{% if missing %}`: is the error strict, and does it name the variable? |
| Whitespace | `{%- -%}`, and the blockquote pattern `> a`, `{% if x %}`, `> b`, `{% endif %}` |
| Raw | `{% raw %}` around `{{`, including inside a fence |
| Typed Expressions | Evaluate `meta.items` and `meta.items \| first` to a real array or value, for `<List items={meta.items} />` |
| Text Expressions | Evaluate `{props.name \| upcase}` by wrapping it in `{{ }}`. What happens if the text contains `}}` or `%}`? |
| Custom filters | Add one that quotes a value safely as a tag attribute |
| YAML to Liquid | Nested maps, lists, numbers, booleans, and null, through `serde-saphyr` |
| Scope | Are `meta.x` and `x` together workable? Is `{% include %}` an error when partials aren't configured? |
| Error locations | A Liquid error, and separately a bad tag produced by a loop's third iteration: can the message point somewhere useful? |

**Pass:**

- Undefined variables are reported by name.
- Typed values work.
- `{% raw %}` and whitespace control behave as documented by Shopify's Liquid.
- Errors give a usable location, or the rendered snippet does the job.

**If it fails:** Add custom tags or filters, restrict the syntax of Expressions, show rendered snippets in errors, or, for a fundamental gap, patch or fork the crate.

**Findings:** _Not started._

## Spike 4: Typst embedding

**Question:** Can `mdx2pdf-render`'s job be done with Typst in-process, offline, and deterministically?

**Why it matters:** This confirms or replaces the Renderer. It's also where binary size and speed get measured.

**Method:** A small binary with a `World` holding in-memory source, embedded fonts, a confined file root, and no packages or network. A walker over the `markdown` tree emits Typst source, which goes through `typst` and `typst-pdf`.

**Fixtures:**

- Headings, nested lists, task lists, tables, code blocks, quotes, footnotes, links, thematic breaks, and PNG and SVG images.
- Adversarial text: `$5 and $6`, `Issue #42`, `@name`, `<label>`, `*`, `_`, backticks, `=` at the start of a line, `\`, and non-Latin scripts.
- A long document of about 50 pages.
- Page Settings: A4 and Letter, landscape, margins, base font and size, and page numbers.

**Measure and record:** stripped release binary size, clean build time, and rendering time for 1 and 50 pages. A person then sets the targets from these numbers.

**Pass:**

- `pdftotext` finds every fixture string, with adversarial text unchanged.
- SHA-256 is identical across two runs and in a second clean container.
- No system fonts or network are used.
- Page Settings visibly apply.

**If it fails:** Try Fulgur with the same fixtures, emitting HTML from the same tree. Only stages 7 and 8 would change. Test Fulgur's determinism first, since its README warns that output can vary across machines. If both fail, a person needs to revisit the self-contained binary requirement.

**Findings:** _Not started._

## Spike 5: End-to-end Expansion

**Question:** Does the `instantiate` design in the [technical design](technical-design.md#components) hold together from Project to Intermediate Markdown?

**Why it matters:** The first four spikes test the parts. This one tests how they interact, which is where most of the [known hazards](technical-design.md#known-hazards) are.

**Depends on:** Spikes 1–3. It stops at Intermediate Markdown, with no PDF needed.

**Method:** The smallest version of the pseudocode, run against fixture Projects, with `insta` snapshots.

**Fixtures:**

- The PRD's example, which must produce exactly the Markdown shown there.
- Nested Components, with a Component passing children on to another.
- `<Content />` inside a quote, a list item, and a table cell, and multi-paragraph children in each.
- Children containing Expressions, which must use the caller's scope.
- `{% raw %}{{ x }}{% endraw %}` in children, which must stay literal.
- Loops that generate Components.
- Tags alone on a line, and tags mid-sentence, including one that wrongly expands to several blocks.
- Direct and indirect recursion.
- Every remaining row of the known hazards table.

**Pass:**

- All fixtures match their expected output, or give the expected error.
- No MDX nodes survive.
- Re-parsing the output gives the same tree as Expansion produced.

**If it fails:** Revisit the expansion design before starting the real crates. A `<Content />` in a table cell that can't work may just need a documented limit, but a failure in scope or container handling means rethinking the design.

**Findings:** _Not started._

## After the spikes

1. Record findings above, and update the [technical design](technical-design.md) and [PRD](prd.md).
2. Write an ADR for the Renderer.
3. Have a person set performance and size targets from the Spike 4 numbers.
4. Delete `spikes/` and start [Milestone 1](technical-design.md#delivery).
