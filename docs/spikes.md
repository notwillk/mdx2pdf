# Spikes

Short experiments to test the riskiest assumptions in the [technical design](technical-design.md) before building on them. Status as of 2026-11-02: Spike 1 failed, Spike 1b passed every bar except exact output and its recommendations were accepted, and Spikes 2–5 haven't started.

## Ground rules

- **Throwaway code.** Each spike lives in `spikes/NN-name/` and gets deleted once its findings are recorded. The exception is `spikes/01-serializer/`, which stays until its test suites move into the real tests, because the findings link to it. Nothing graduates into the real crates without being rewritten.
- **Timeboxed.** When time runs out, record what's known and decide whether more is worth it.
- **Objective result.** Each spike has a pass condition set in advance. Change a bar only by writing down why.
- **Findings go here.** A result that settles a hard-to-reverse decision also gets an [ADR](adr/), and the design gets updated.

**Prerequisite:** Rust 1.92 or later, which the Typst crates need. `.devcontainer/install.sh` installs it each time the container starts, if it's missing.

## Overview

| # | Question | Timebox | If it fails |
|---|---|---|---|
| [1](#spike-1-markdown-serializer) | Can `mdast_util_to_markdown` round-trip reliably? | 0.5 day | **Failed.** Go to Spike 1b |
| [1b](#spike-1b-event-based-serializer) | Can `pulldown-cmark-to-cmark` serialize our trees through a small adapter? | 0.5 day | Write our own serializer. **Not needed:** it passed every bar except exact output |
| [2](#spike-2-dialect-parsing) | Does `markdown` parse the Dialect? | 0.5 day | Work around the gaps, or change parser |
| [3](#spike-3-liquid-behavior) | Does `liquid` behave the way the design needs? | 1 day | Custom filters, weaker errors, or a fork |
| [4](#spike-4-typst-embedding) | Can Typst embed cleanly and deterministically? | 1–2 days | Try Fulgur |
| [5](#spike-5-end-to-end-expansion) | Does Expansion work end to end? | 1–2 days | Revisit the expansion design |

Run the remaining spikes in this order:

1. **Spike 2** comes next, because the serializer's adapter depends on the parser's tree types, and Spike 1b found inputs that panic the parser.
2. **Spikes 3 and 4** can run in either order.
3. **Spike 5** comes last, because it needs a working serializer and normalization.

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

**Findings (2026-11-02): failed.** These findings propose writing our own serializer, in about 2–3 days. The [evaluation](#evaluation-of-spike-1) below tries an alternative first. Harness and raw output are in `spikes/01-serializer/`, and `results/record/misses.txt` has every miss with its input and output.

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

### Evaluation of Spike 1

A separate review (2026-11-02) checked these findings against the harness, the saved results, and the crate's source, without re-running the harness.

**The failure is real.** Every count, all 43 spec misses, and both bugs check out. The verdict doesn't depend on the 99% bar, because R14 requires tables, footnotes, strikethrough, and task lists, and the crate fails all four. Rejecting a fork is sound too, since all the patches combined only reach 94.17%. Two results support the rest of the design:

- The tree-based expansion design holds, because the `\<div>` and `    total` hazards it was chosen for both pass.
- The `markdown` parser had no panics or parse errors in Spike 1's suites. That didn't hold up: [Spike 1b](#spike-1b-event-based-serializer)'s random documents found inputs that panic it.

**"Write our own" was premature.** The spike left three things untested:

1. **The production path.** Stage 3 parses the Dialect and Stage 6 parses GFM, but only 15 hand-picked cases took that path. The spec suites use one grammar on both sides. The headline 93.40% is the CommonMark run, and the GFM run is 93.25%.
2. **Trees shaped like Expansion output.** Every hand-built case has a single text value in one container. None has adjacent nodes or content from more than one file.
3. **Any alternative except a fork.** See [Spike 1b](#spike-1b-event-based-serializer).

**What changed as a result**, all in the [technical design](technical-design.md#serialization):

- Trees get normalized before serializing. Resolving references there also clears 9 of the 20 reachable spec misses (194, 554, 558, 559, 564, 573, 576, 585, 589), and takes Stage 5 from 24 node kinds to 21.
- The round trip is checked on every run, not only in tests.
- Literal autolinks are off.

**One limit on these results:** the patches behind the three patched runs weren't saved, so those runs can't be reproduced. The verdict doesn't depend on them.

## Spike 1b: Event-based serializer

**Question:** Can `pulldown-cmark-to-cmark` serialize our trees, through a small adapter, without changing their meaning?

**Why it matters:** Writing our own means owning roughly 600–900 lines of escaping and delimiter logic. An adapter from our tree to this crate's events would be about 150–250 lines. Both figures are judgment.

Its [README](https://github.com/Byron/pulldown-cmark-to-cmark) claims that all 652 CommonMark examples keep the same events after a round trip, backed by a differential suite of 91,446 generated inputs. It also claims support for tables, footnotes, strikethrough, and task lists, and that it returns `Error::Unrepresentable` rather than silently changing some delimiter blocks. None of this has been run here.

**Method:** Add the adapter to the existing harness in `spikes/01-serializer/`, with `gfm_autolink_literal` off and [normalization](technical-design.md#normalization) applied. Re-parse the output under both `markdown` and `pulldown-cmark`, and report each separately. Note that `code_block_token_count` defaults to 4, so set it to 3.

**Inputs:**

- The five existing suites.
- All 624 spec inputs that parse in the Dialect, through the production path: Dialect, normalize, serialize, GFM.
- Expansion-shaped trees: adjacent nodes, empty values, grafted blocks, and several files.
- Randomized text values in every container.
- The PRD's example, which must produce exactly the Markdown shown there.

**Watch for:**

- **Edited events.** Its guarantee is for parser-produced events, and its README says it doesn't check whether arbitrary edited events are representable. Expression values and text containing line breaks are the likeliest weak spots.
- **Parser differences.** It targets `pulldown-cmark`, and we currently re-parse with `markdown`, so the cross-parser result is the real test.

**Also find out:**

- Which `pulldown-cmark` version does it pin, and how active is it?
- If the output re-parses cleanly under `pulldown-cmark`, should Stage 6 move there? That would leave `markdown` only in Stage 3, the one job no other parser does, which matters because its repo has had no commits since April 2025. Decide before Spike 4, because it changes what the Typst emitter reads.

**Pass:** Spike 1's bars (equal trees on all curated cases, at least 99% of spec examples, and no panics), the PRD example's exact output, and an explanation for every miss.

**If it fails:** Write our own, as Spike 1's findings propose. Their 2–3 days covers the listed suites. With normalization excluded but these extra tests included, plan on 4–5 days, which is also judgment.

**Findings (2026-11-02): passed every bar except exact output. Recommendation: adopt it.** The harness is `spikes/01-serializer/src/bin/events.rs`, and `results/record-1b/` has the raw output. Reproduce it with `cargo run --release --bin events -- record-1b`. Re-running Spike 1's binary also reproduced its record byte for byte.

| Bar | Result |
|---|---|
| At least 99% of spec examples | **Pass.** 99.69%: 650 of 652, against 93.40% for `mdast_util_to_markdown` |
| All curated cases equal | **Pass.** 47 of 47, against 33 of 47 |
| No panics | **Pass** for the serializer, with none in any suite. The 29 panics all came from the `markdown` parser |
| The PRD example's exact output | **Fail.** The tree matches, but the text doesn't |
| An explanation for every miss | **Pass** |

The two spec misses are example 173, which involves `Html` and can't reach Stage 5, and example 321, where only tight versus loose differs.

**The new suites:**

| Suite | Result |
|---|---|
| Production path: the 624 spec inputs that parse in the Dialect | 623 of 624 (99.84%). The miss is example 321 |
| Dialect cases, hand-built trees | 15 of 15, 59 of 59 |
| Expansion-shaped trees, through a small mock of `instantiate` | All 40 behaved as expected: 36 round-trip, Expansion rejects 3, and the round-trip check catches 1 |
| Random values with no edge whitespace or line breaks | 220,000 of 220,000 (20,000 values in each of 11 containers) |
| Random values with whitespace and line breaks, then trimmed | 97.44–99.69% per container |
| Random documents, GFM | 49,104 of 50,000 (98.21%) |
| Random documents, Dialect | 45,748 of 46,546 (98.29%) |

**Random document misses:**

| Miss | GFM | Dialect | Effect |
|---|---|---|---|
| Tight versus loose only | 747 | 659 | Spacing only, mostly lists of empty items |
| Real mismatch | 102 | 96 | Caught by the round-trip check. In the GFM run: 63 consecutive hard breaks, 20 multi-line code spans in a setext heading, and 19 unusual combinations such as `- * ***` |
| `Unrepresentable` | 26 | 22 | The crate returns an error |
| Malformed tree | 6 | 7 | A `markdown` bug: a list directly inside a list. The adapter rejects it |
| Parser panic | 15 | 14 | A `markdown` bug, described below |

So every miss either changes only spacing or produces an error. **None reaches a PDF silently**, provided the round-trip check runs. The random value misses are the same kind: a literal `\` just before a line break (the crate writes `\&#10;`, which reads as literal text), and values with a non-breaking space at the edge of emphasis (`Unrepresentable`).

**Exact output.** The crate hard-codes its block quote prefix, with no option to change it:

```text
 > 
 > **ALPHA**
 > 
 > Review **Alpha** before Q3 Report ships.
```

This is valid Markdown with the same tree as the PRD's example, but it isn't the same text. `results/quote-patch/quote.patch` changes 5 lines of the crate, produces the PRD's exact text, and doesn't change the result of any suite. So the options are to relax PRD success criterion 1 to "the same document", ask upstream for an option, or carry that patch. **Decision:** relax the criterion. Asking upstream is still to do. A cosmetic difference in a file only `--emit-md` shows shouldn't decide between this and owning 600–900 lines.

**The `markdown` parser panics on small inputs.** There were 29 panics in 96,546 random documents, at three locations. `results/record-1b/parser-panics.txt` has the reduced inputs, and `src/bin/reduce.rs` produced them.

| Input | Options that panic |
|---|---|
| `` "1. ```\n- x\n" ``, `` "- ```\n1. x\n" ``, `"- ~~~\n1. x\n"` | Stock CommonMark and GFM |
| ``"\\\n-\n=\n`\n-"`` | Stock CommonMark and GFM |
| `"<>]\n-\n"` | Dialect |

The first row matters most: **an unclosed fence at the end of a list item, followed by a different kind of list.** An unclosed fence is an ordinary typo, and closing it avoids the panic. A nearby shape, `` "# t\n  - ```<\n* x|\n" ``, returns a malformed tree without panicking. `pulldown-cmark` had no panics on 100,000 random documents or on any serialized output.

**Should Stage 6 move to `pulldown-cmark`? Decision: yes ([ADR 0003](adr/0003-two-markdown-parsers.md)).** A Markdown Input then never touches `markdown`, and `markdown` would remain only in Stage 3, the one job nothing else does. Under `pulldown-cmark`, the production path scores 619 of 624 (99.20%), and all 5 misses are code spans containing line breaks, which the two parsers store differently. Random documents score 98.47% and 98.59%, slightly above `markdown`. The cost is two parsers instead of one, and the Typst emitter reads events rather than a tree. The one-parser argument was already weak, because the Dialect and Markdown grammars differ anyway.

**Also found out:**

- **Literal autolinks:** turning them on drops the GFM spec run to 97.70%, curated cases to 43 of 47, and random documents to 84.61%. That confirms keeping them off.
- **Size:** the adapter is about 310 lines and normalization about 200, both with comments. The adapter came in above the 150–250 estimate.
- **Maintenance:** 23.0.0 came out on 2026-09-29, after 22.0.3 and 22.0.2 the day before and 22.0.1 in August. It requires `pulldown-cmark` 0.13. It went from 21.1.0 to 23.0.0 in under a year, so upgrades may need work, and it should be pinned exactly.
- **Two of my predictions were wrong.** The evaluation said `x<B />y`, where `B` expands to `*(a)*`, has no Markdown form. The crate spells it `&#120;*(a)*&#121;`, which round-trips exactly. Adjacent emphasis also works, as `*a*_a_`. Only the link inside a link really has no form.
- **More normalization is needed:** see the [technical design](technical-design.md#normalization). The spike added rules for list looseness, hard breaks, block edges, short table rows, and trimming Expression values.
- **A grafting bug the check caught:** two paragraphs placed in a tight list item merged into `onetwo`, which the round-trip check reported.

**Worth reporting upstream** (not reported yet): for `markdown`, the panics above, and `spread` being set when two blank lines follow a list. For `pulldown-cmark-to-cmark`, a hard break at the start of a paragraph or two in a row (written as a blank line), a table cell ending in `\`, a `\` before an encoded line break, a multi-line code span in a heading, the `- * ***` case, and an option for the block quote prefix.

**Not tested:** real Expansion, because the mock has no Liquid and only a minimal version of Props. Also stack depth on deep nesting, and performance.

### After the decisions

Both recommendations were accepted on 2026-11-02, so Spike 1b was measured again with the round-trip check comparing `pulldown-cmark` events, as the design now does. Reproduce it with `cargo run --release --bin events -- stage6 --stage6-events`. The output is in `results/stage6/`.

| Suite | Equal under `pulldown-cmark` |
|---|---|
| Production path | **624 of 624** |
| Curated, Dialect, and hand-built cases | 47 of 47, 15 of 15, 59 of 59 |
| All spec examples | 648 of 652. All 4 misses involve `Html`, which can't reach Stage 5 |
| Expansion-shaped trees | All 40 behaved as expected |
| Random values with no edge whitespace or line breaks | 219,690 of 220,000. All 310 misses are a table cell ending in `\` |
| Random documents, GFM and Dialect | 49,531 of 50,000 (99.06%), and 46,132 of 46,546 (99.11%) |

The random GFM misses are 353 table cells (mostly ending in `\`), 63 consecutive hard breaks, 19 `Unrepresentable`, 15 parser panics, 13 unusual combinations, and 6 malformed trees. As before, each of these produces an error rather than a changed PDF.

**A correction.** The first version of these findings, and the summary given at the time, said a dummy definition, `[cmark-loose-1]: <>`, appeared only on odd random documents. That was wrong, because the probe only tried one blank line. **A list followed by two blank lines triggers it**, and a tight list could also turn loose, which would change the PDF's spacing. The cause is `markdown` setting `spread` in that case. Working out tight or loose from source lines fixes it: the dummy definition now appears nowhere, including in random documents, and tight lists stay tight.

**Two more fixes from this run:**

- **Ordered lists:** by default the serializer numbers every item `1.`, so the design turns on `increment_ordered_list_bullets`.
- **Code spans:** the adapter now applies CommonMark's rule for line breaks in a code span, because `markdown` leaves it undone.

**Final size:** the adapter is about 340 lines and normalization about 250, both with comments. The `record-1b` run above was made before these changes and stays reproducible, because they're behind the `--stage6-events` flag.

## Spike 2: Dialect parsing

**Question:** With JSX and expressions on and ESM off, does `markdown` give us the tree the design needs?

**Why it matters:** The docs say each construct has its own flag, but don't say whether this combination works. And `Constructs::mdx()` also turns off autolinks, indented code, and HTML, which makes the Dialect differ from Markdown.

**Method:** Build a custom `Constructs` with `mdx_jsx_flow`, `mdx_jsx_text`, `mdx_expression_flow`, `mdx_expression_text`, and the GFM constructs on, and `mdx_esm`, `html_flow`, `html_text`, `code_indented`, `autolink`, and `gfm_autolink_literal` off. Print the tree for each input below.

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
| Random and malformed input, including `` "1. ```\n- x\n" `` | Returns `Err`, or panics in a way a guard can catch |

**Also find out:** Is the crate's lack of commits since 2025-04-23 a practical problem? Check whether the open PRs include bugs we'd hit, and how hard a fork would be.

**Pass:** Correct nesting, Expression text preserved, no panic that escapes a `catch_unwind` guard, and a written list of how the Dialect differs from Markdown.

This bar was "no panics". It changed because Spike 1b already found inputs that panic this parser, and Stage 3 has no alternative parser. So the question is now whether a guard turns those panics into ordinary errors, and whether the malformed trees it can return are detectable.

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

**Method:** A small binary with a `World` holding in-memory source, embedded fonts, a confined file root, and no packages or network. A walker over `pulldown-cmark` events emits Typst source, which goes through `typst` and `typst-pdf`.

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

**Depends on:** Spikes 1b, 2, and 3, plus a working serializer and normalization. It stops at Intermediate Markdown, with no PDF needed.

**Method:** The smallest version of the pseudocode, run against fixture Projects, with `insta` snapshots.

**Fixtures:**

- The PRD's example, which must produce the same document as the Markdown shown there.
- Nested Components, with a Component passing children on to another.
- `<Content />` inside a quote, a list item, and a table cell, and multi-paragraph children in each.
- Children containing Expressions, which must use the caller's scope.
- `{% raw %}{{ x }}{% endraw %}` in children, which must stay literal.
- Loops that generate Components.
- Tags alone on a line, and tags mid-sentence, including one that wrongly expands to several blocks.
- Direct and indirect recursion.
- Two Components defining the same reference label with different URLs, and a Component with a footnote used twice.
- A tree with no Markdown form, which must give an error: `[<L />](/c)`, where `L` expands to a link.
- Every remaining row of the known hazards table.

**Pass:**

- All fixtures match their expected output, or give the expected error.
- No MDX nodes survive.
- Re-parsing the output gives the same `pulldown-cmark` events as the normalized tree.

**If it fails:** Revisit the expansion design before starting the real crates. A `<Content />` in a table cell that can't work may just need a documented limit, but a failure in scope or container handling means rethinking the design.

**Findings:** _Not started._

## After the spikes

1. Record findings above, and update the [technical design](technical-design.md) and [PRD](prd.md).
2. Write an ADR for the Renderer.
3. Have a person set performance and size targets from the Spike 4 numbers.
4. Delete `spikes/`, once Spike 1's suites have moved into the real tests, and start [Milestone 1](technical-design.md#delivery).
