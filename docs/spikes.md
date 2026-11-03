# Spikes

Short experiments to test the riskiest assumptions in the [technical design](technical-design.md) before building on them. Status as of 2026-11-02: Spike 1 failed, Spike 1b passed every bar except exact output and its recommendations were accepted, Spike 2 passed with required changes and its fork recommendation was accepted, Spike 3 passed three of its four bars with required changes and its patch recommendation was accepted, Spike 4 passed except that no second container was available, and Spike 5 hasn't started.

## Ground rules

- **Disposable code, kept as evidence.** Each spike lives in `spikes/NN-name/`, committed with its results so the findings can be checked and reproduced. Nothing graduates into the real crates without being rewritten.
- **Timeboxed.** When time runs out, record what's known and decide whether more is worth it.
- **Objective result.** Each spike has a pass condition set in advance. Change a bar only by writing down why.
- **Findings go here.** A result that settles a hard-to-reverse decision also gets an [ADR](adr/), and the design gets updated.

**Prerequisite:** Rust 1.92 or later, which the Typst crates need. `.devcontainer/install.sh` installs it each time the container starts, if it's missing. Turn off core dumps with `ulimit -c 0` before running Spike 2, for the reason in [`spikes/README.md`](../spikes/README.md).

## Overview

| # | Question | Timebox | If it fails |
|---|---|---|---|
| [1](#spike-1-markdown-serializer) | Can `mdast_util_to_markdown` round-trip reliably? | 0.5 day | **Failed.** Go to Spike 1b |
| [1b](#spike-1b-event-based-serializer) | Can `pulldown-cmark-to-cmark` serialize our trees through a small adapter? | 0.5 day | Write our own serializer. **Not needed:** it passed every bar except exact output |
| [2](#spike-2-dialect-parsing) | Does `markdown` parse the Dialect? | 0.5 day | Work around the gaps, or change parser. **Not needed:** it passed, with required changes |
| [3](#spike-3-liquid-behavior) | Does `liquid` behave the way the design needs? | 1 day | Custom filters, weaker errors, or a fork. **Mostly:** custom filters and a small patch, with `{% if missing %}` still silent |
| [4](#spike-4-typst-embedding) | Can Typst embed cleanly and deterministically? | 1–2 days | Try Fulgur. **Not needed:** Typst is confirmed ([ADR 0006](adr/0006-typst-is-the-renderer.md)) |
| [5](#spike-5-end-to-end-expansion) | Does Expansion work end to end? | 1–2 days | Revisit the expansion design |

Run the remaining spikes in this order:

1. **Spike 5** is the last one. It needs a working serializer and normalization.

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

1. **Non-ASCII text (`state.rs:513`).** `value.chars().nth(*position)` uses a byte offset as a character index. After any non-ASCII text, it reads the wrong character and encodes that instead, or slices inside a character. This is the more serious bug. These findings said no open PR addressed it, but Spike 2's review of the PR list found [#181](https://github.com/wooorm/markdown-rs/pull/181), which targets it, and [#272](https://github.com/wooorm/markdown-rs/pull/272) for the task list bug. Neither was tested, and neither changes the verdict.
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

**Findings (2026-11-02): passed, with required changes. Recommendation: carry a small fork.** **Decision:** accepted on 2026-11-02 ([ADR 0004](adr/0004-fork-the-markdown-parser.md)). Numbers are from `spikes/02-dialect/results/`, and later runs of the deterministic suites matched exactly. The [evaluation](#evaluation-of-spike-2) below corrects some of what follows.

| Bar | Result |
|---|---|
| Correct nesting | **Pass.** 16 of 16 cases, including tags in lists, quotes, tables, and other tags, with block children |
| Expression text preserved | **Pass with the callback below.** Without it, `{x \| replace: "}", ""}` silently closes early. Multi-line Expressions have one small limit |
| No panic escapes the guard | **Pass.** None of 1,591 panics in 250,000 documents escaped, and a known document parsed the same afterward. The parser also survives nesting 100,000 deep |
| Written list of differences | **Pass.** See below |

The table suite had 79 cases with results written in advance, and 73 matched. The 6 surprises: `<A /> <B />` is already two blocks, the two prose cases and the header-row table below, the continuation-line limit below, and one case where the predicted behavior was right but the expected text had a typo.

**The panics are more ordinary than Spike 1b suggested:**

| Input | Stock 1.0.0 | Why it matters |
|---|---|---|
| `<Card title="AT&T" />`, `<Card title="a&b" />` | Panic at `character_reference.rs:196` | **An `&` followed by letters at the end of an attribute.** A company name can trigger it, including one from a Liquid value. `"a & b"`, `"a&b c"`, and `"AT&amp;T"` are fine |
| `<A>hi`, `---`, `</A>` on separate lines | Panic at `to_mdast.rs:1277` | An unclosed tag before a setext underline |
| `` - ``` ``, then `1. x` | Panic at `to_mdast.rs:1817` | An unclosed fence, as Spike 1b found. `` * ``` ``, then `-`, gives a malformed tree |
| `a`, `-`, `--`, `a`, `-` | Panic at `to_mdast.rs:193` | Adjacent setext headings |
| `[](a (a` + space, newline, `))` | Panic at `subtokenize.rs:149` | A link title ending in a space and a line break |

The guard catches all of these, but a caught panic has no location, while all 14 of the table suite's parse errors do. So the guard alone means `AT&T` in an attribute reports only "the parser crashed in this file".

**Silent failures the guard can't help with.** Each of these produces a tree, not an error, so each needs its own handling:

| Input | Result | Handling |
|---|---|---|
| `<Card title="x"`, with the `>` missing | **Prints as literal text.** Also `<Card` and `a <Card title="x" b`, and `a <Card b={1}` prints `a <Card b=` plus an Expression | A check on text nodes, untested (see below) |
| `A<B and more`, `if a<b then` | Stays text, while `A<B, C`, `List<String>`, and `Vec<u8> here` are errors | Same check, for uppercase tag names. A lowercase `a<b` stays text |
| A table whose header has `{x \| upcase}`, when the pipe makes the cell count differ | **The whole table becomes a paragraph**, printing `\|---\|` | A check on paragraphs, untested. In a body row it's a clear error instead, at the cell's `{` |
| `{x \| replace: "}", ""}` | The Expression is `x \| replace: "`, followed by the literal text `", ""}` | The callback below |

**Expressions:**

- **Single-line text is exact,** including `|` and surrounding spaces.
- **A small callback fixes strings.** Setting `mdx_expression_parse` to a roughly 15-line scanner that tracks quotes and braces makes `}` and `{` inside strings work, in text and in attributes. It changed only the intended case among the 79. The trade-off is that `{it's}` becomes an error, where before it was an Expression that Liquid would reject anyway, and that error points at the end of the file.
- **Multi-line limit:** continuation lines lose up to two leading spaces beyond any container prefix, so `{a`, `    b}` gives `a`, `  b`. Liquid ignores that whitespace except inside a string spanning lines, so documenting it is enough.
- **In table cells, an escaped pipe stays escaped.** `{x \| upcase}` gives the value `x \| upcase`, so Expansion must turn `\|` back into `|` before Liquid sees it, in cell Expressions and in attributes on tags in cells. This handling is untested.
- **`{/* comment */}`** is an Expression with the value `/* comment */`, so it works as a comment if Expansion drops it, as the PRD says it should. The parser's own error for `<!-- x -->` suggests this syntax too.

**Attributes** appear as expected: `b={1}` is an expression value, `c="x"` a decoded literal, `d` has no value, and `{...b}` is an expression attribute that the design rejects. Attribute nodes have no positions, so errors about one have to point at its tag. Tag and Expression nodes point at their source: 52 of 52.

**Inline and block tags.** The design's assumption holds, with one refinement. `<Note>hi</Note>` alone on a line is inline in a paragraph. But `<A /> <B />` and `<A /> {x}` are already blocks, so the parser does some unraveling itself. `<A>x</A> <B>y</B>` stays a paragraph, and so does a tag on each of two lines, which has a `"\n"` text node between them, so unraveling must treat line breaks as whitespace.

**Detecting ESM.** The method assumed ESM off plus a rejecting `mdx_esm_parse`, but the parser only consults that callback with `mdx_esm` on (`mdx_esm.rs:48-50`), so these are the real options:

| Detector | Score of 19 | Misses |
|---|---|---|
| Top-level paragraph at column 1 starting with `import ` or `export ` | 16 | Also flags `import duties rose sharply`, `export the file as a PDF`, and an indented `import` |
| ESM on, with a rejecting callback | 17 | Also flags the same two prose lines |
| **The first check, reading the source line and requiring it to look like a statement** | **19** | None |

**Use the third, tightened by the [evaluation](#evaluation-of-spike-2).** It scored 17 on its first version and was fixed after seeing the results, so its 19 of 19 was on its own test set. A statement is now `import` followed by a quote, `{`, `*`, or ` from ` and then a quote, or `export` followed by `const`, `let`, `var`, `function`, `class`, `default`, `async`, `{`, or `*`. If it misfires, the escape is `&#105;mport` or `&#101;xport`. A backslash isn't an escape here, because `\import` prints the backslash.

**How the Dialect differs from Markdown**, across all 652 spec examples: 498 give the same tree, 81 a different tree, and 73 an error.

| Author writes | In Markdown | In the Dialect | Examples |
|---|---|---|---|
| A closed HTML block such as `<table>…</table>` | Raw HTML | **Component-like tags with parsed children.** Then rejected because they're lowercase (R8) | 30 different |
| Unclosed HTML such as `<div>`, `<a href="x">`, or `<br>` | Raw HTML | Error: "Expected a closing tag" | 42 errors with HTML off, some with other causes |
| `<!-- x -->`, `<?php ?>`, `<![CDATA[ ]]>` | Raw HTML | Error at the `!` or `?`. The comment error suggests `{/* text */}` | Included above |
| Text indented 4 spaces or a tab | Code block | **A paragraph, silently** | 50 different, 4 more combined with HTML |
| `<https://a.b>`, `<a@b.c>` | Autolink | Error, suggesting `[text](url)` | 17 errors |
| `<` where Markdown reads text or a link destination, such as `[a](<b c>)` or `<a b c>` | Text or part of the URL | Error | 14 errors |
| `{x}` | Text | An Expression | None in the spec |
| `a<1`, `3 <4` | Text | Error | Not in the spec's differences |
| `a < b`, `a <b`, `a > b`, a lone `}` | Text | Text | Same |

The last different tree is example 91, where a lowercase tag with a multi-line attribute parses as a tag, which R8 rejects anyway. The counts add up to the totals: 30, 50, and 1 different, and 42, 17, and 14 errors.

**Depth and scaling:**

- **The parser didn't overflow.** Parsing and dropping a tree nested 100,000 deep works on the default 8 MB stack for quotes, inline tags, and block tags, though it takes two to three minutes (`results/evaluation/depth.txt`). Links and braces also finished at 100,000. Lists, ordered lists, and emphasis reached 10,000, then hit the 20-second limit, so they're unproven beyond that.
- **Recursive walks do overflow.** The spike's validator walked 50,000 levels, but aborted the process at 100,000 on quotes, inline tags, and block tags, and `catch_unwind` can't stop that. A 1 GiB thread contained it for quotes and inline tags. So Expansion, which is recursive and has bigger frames, needs a depth limit, as the design already assumes. Its frames haven't been measured, so the limit should be set well below these numbers.
- **Parsing is superlinear, and the guard won't stop a slow parse.** This is reported only:

| Input size | Prose | Tags | Expressions | List | Brackets | Table rows |
|---|---|---|---|---|---|---|
| 64 KB | 0.008s | 0.075s | 0.073s | 0.102s | 0.097s | 0.602s |
| 256 KB | 0.055s | 0.450s | 0.616s | 1.149s | 2.200s | 13.201s |
| 1 MB | 0.435s | 8.843s | 13.036s | 27.374s | 42.292s | Over 60s |

These are for stock 1.0.0, and the fork and the callback barely change them (`results/evaluation/scaling.txt`). Normal documents are well below where this matters, and each file parses separately. A generated table is the likeliest way to hit it: a 256 KB table takes 13 seconds. Liquid loops can generate one, so Spike 5 should keep this in mind. Open PRs target scaling, including [#197](https://github.com/wooorm/markdown-rs/pull/197) and [#238](https://github.com/wooorm/markdown-rs/pull/238). They weren't tested.

**Is the stale crate a practical problem? Yes, but a small fork solves it.** Upstream's last commit is still 2025-04-23, while fixes keep arriving as open PRs. Of the panic PRs reviewed, these six apply cleanly together:

| PR | Commit | Fixes |
|---|---|---|
| [#265](https://github.com/wooorm/markdown-rs/pull/265) | `e26c525bd7` | `AT&T` in an attribute, a one-line bounds check |
| [#267](https://github.com/wooorm/markdown-rs/pull/267) | `ccc6ed01ea` | A list after an item ending in unclosed code, including the malformed trees |
| [#219](https://github.com/wooorm/markdown-rs/pull/219) | `a1129c332d` | A setext underline after an unclosed tag, which becomes a located error |
| [#210](https://github.com/wooorm/markdown-rs/pull/210) | `79d3a45e94` | Adjacent setext headings |
| [#205](https://github.com/wooorm/markdown-rs/pull/205) | `44542f924d` | List item positions around code |
| [#207](https://github.com/wooorm/markdown-rs/pull/207) | `d2efc4814c` | Whitespace in link titles and labels |

Together they're 91 lines added and 23 removed across six source files, on `1506572`, with no manifest or `build.rs` changes. **The fork is the patches in `results/fork/`, applied in this order, not the PRs.** The commits above are each PR's head, and they don't reproduce the fork on their own: #219's is only a `debug_assert` change, with the fix in its other commit, and #210 and #207 are stacked on other branches, at about 80 commits and 22,000 added lines each. #265, #267, and #219 are whole PRs with their tests, and the other three are only the source file from the commit above.

| 250,000 documents | Stock 1.0.0 | Fork |
|---|---|---|
| Caught panics | 1,591 | **5** |
| Malformed trees | 15 | **0** |
| Position problems | 12 | 4 |

The 5 left share one shape, `to_mdast.rs:1842`, such as `a||`, `-|-`, `1. <`: a table header, then a list containing an unclosed `<`. The guard still handles it. The 652 spec comparisons didn't change at all, and the other differences in the suites are panics becoming trees or errors, plus the malformed tree being fixed.

**The fork's test suite wasn't run,** so that evidence is ours, not upstream's. Running it was denied by this environment's permission check. It needs the `swc_core` dev-dependency, and PR #271's author reports that a related dependency doesn't compile, with [#230](https://github.com/wooorm/markdown-rs/pull/230) open to fix CI. Do this before adopting the fork.

**Estimate:** about half a day to set up the fork, run its tests, and pin it by commit, which is judgment. It costs little to carry, because upstream isn't moving, so there's nothing to rebase onto. The reasons to prefer it to the guard alone are `AT&T` and the malformed trees. The alternative is the guard plus a hint saying to check for `&` and unclosed fences, which is a poor message for ordinary mistakes.

**Not tested:**

- The proposed checks: unterminated tags, tables that became paragraphs, and `\|` unescaping. Spike 5 now covers them.
- Trees that are well formed but wrong. The validator only checks shape and positions.
- The other open PRs, such as [#268](https://github.com/wooorm/markdown-rs/pull/268) and [#269](https://github.com/wooorm/markdown-rs/pull/269), which might let the design drop its own list tightness work.

**Worth reporting upstream** (not reported yet): `<A b="AT&T" />` isn't specifically mentioned in #265's title, and a note there would show how ordinary it is. Also a table header followed by a list with an unclosed `<`, which no PR we reviewed covers.

### Evaluation of Spike 2

A separate review (2026-11-02) re-ran the suites, rebuilt the fork from the saved patches, compared those patches with upstream, and tested what the spike left out. Its evidence is in `spikes/02-dialect/results/evaluation/`.

**The results hold, and the fork is still the right call.** Stock and fork both reproduce byte for byte, and the counts above check out. Three new results support them:

- **With the expression callback on**, which is what Stage 3 uses and the spike's robustness runs didn't, the fork still has 5 panics and 0 malformed trees in 250,000 documents.
- **Scaling** barely changes with the fork or the callback. The spike only measured stock.
- **The depth claims are true:** quotes parsed 100,000 levels in about 114 seconds and inline tags in about 131, and a recursive walk at that depth aborts the process unless it has a 1 GiB stack. Those runs weren't saved at the time, and `results/record/deep.txt` shows timeouts because it used a 20-second limit. Block tags also finished at 100,000, in about 182 seconds, where the spike had stopped at 50,000.

**What it changed:**

| Problem | Evidence | Change |
|---|---|---|
| The commits in ADR 0004 couldn't rebuild the tested fork | `upstream-prs.txt` | The ADR now builds from the saved patches |
| The ESM check flagged ordinary prose, such as `import duties from China rose sharply` | 5 of 7 new prose lines flagged, in `esm-and-callback.txt` | `from` must be followed by a quote. That still scores 19 of 19 and reports all 8 new statements, and it flags 2 of the 7 prose lines: `export class action lawsuits` and `export default settings are fine` |
| The recommended `\import` escape printed its backslash | Run through the parser | The escape is now `&#105;mport` |
| An open quote in an Expression reports its error at the end of the file, and can swallow later blocks | `esm-and-callback.txt` | Proposed handling added to the design, untested, in Spike 5 |
| Authors weren't told about pipes in table cells, and nothing required the silent failures to become errors | The PRD | Added to R7, and a new R26 |
| Spike 2's crash controls left core dumps holding the session's environment, including tokens | Ten files, about 48 MB | `spikes/.gitignore` keeps them out of commits, and `spikes/README.md` says to run with `ulimit -c 0` |

**Still open:** the fork's own test suite has never run, and neither has the handling for tags with no `>`, tables that became paragraphs, or open quotes. Those are the likeliest remaining ways to get a wrong PDF.

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

**Findings (2026-11-02): three bars passed, one partly, with required changes.** `liquid` can do both jobs, but out of the box **it breaks R23 in two ways**, and one of those needs a patch or a restriction. **Decision:** patch it, accepted on 2026-11-02 ([ADR 0005](adr/0005-sort-liquid-object-keys.md)). Numbers are from `spikes/03-liquid/results/`. Two runs of the deterministic suites differ only in one error message's unordered key list, which is itself the second finding below.

| Bar | Result |
|---|---|
| Undefined variables reported by name | **Partly.** `{{ }}`, comparisons, `for`, `assign`, and `case` all name it. **`{% if missing %}` is silently false**, as Shopify's default is |
| Typed values work | **Pass.** 28 of 28, through a capture filter |
| `{% raw %}` and whitespace control as documented | **Pass, with one limit.** All 5 of Shopify's documented examples match exactly, and raw 7 of 7. Hyphens don't trim tabs |
| Usable error locations | **Pass.** See below. Most Liquid errors have no line number, so we'd have to supply it |

The table suite had 55 cases with results written in advance, and 52 matched. The 3 surprises are all tabs.

**Two ways it breaks byte-identical output (R23):**

1. **Object order is random per process.** `Object` is a `HashMap` with Rust's seeded hasher, so `{% for kv in obj %}` gave 6 different orders in 6 runs on the same input, and `{{ obj }}` prints in the same random order. There's no option to change it. **Replacing the map with a `BTreeMap` is an 11-line change** (`results/object-order/btreemap.patch`, on published `liquid-core` 0.26.11). It gave the same sorted order in 6 of 6 runs, and the only difference across every suite is that a key list in one error message is now sorted. Keys come out alphabetically rather than in YAML order. `liquid-core`'s own 121 tests pass with it. **The wider `liquid` workspace's tests, which hold the conformance suite, haven't run with it**, because this environment's permission check denied that command. Run them before adopting the patch.
2. **`"now" | date` reads the clock,** as do `"today"`, `"NOW"`, and `" now "`. Registering our own `date` filter after the standard library replaces the stock one, with no patch. Ours rejects those words and matched the stock filter on the 5 other inputs tried. It's the only clock use we found by searching the crate, and `{% if d < "now" %}` just compares strings.

**Undefined values.** Every name below comes from the crate's message, such as `Unknown variable, requested variable=missing`:

| Template | Result |
|---|---|
| `{{ missing }}`, `{{ a.b.c }}` | Error naming the variable |
| `{{ meta.missing }}`, `{{ obj.missing }}`, `{{ rows[5].name }}` | Error naming the index, listing what's available |
| `{% if missing == 1 %}`, `{% for i in missing %}`, `{% assign q = missing %}`, `{% case missing %}` | Error naming the variable |
| **`{% if missing %}`, `{% if obj.missing %}`, `{% unless missing %}`, `{% if missing and b %}`** | **Silent: treated as false** |
| **`{{ missing \| default: "x" }}`** | **An error** ([#570](https://github.com/cobalt-org/liquid-rust/issues/570), open), where Shopify's would print `x`. `{{ nullvalue \| default: "x" }}` works |
| `{{ nullvalue }}` | Prints nothing |
| An unknown filter, wrong filter arguments, an unknown tag | Parse error naming it |

The silent `if` is how authors test optional values, so the working default is to keep it and document it. The cost is that `{% if itmes %}` drops content with no error. Both this and `default` are in the PRD's open questions.

**Typed values.** No public function evaluates an expression, so a `__capture` filter keeps the typed value it receives. `{{ meta.items | __capture }}` and `{% assign __v = meta.items %}{{ __v | __capture }}` both got all 28 right, including arrays, objects, numbers, booleans, null, and `first`. Delimiters inside strings are safe: `{"}}"}` and `{name | append: "}}"}` work.

**Each wrapper can be escaped alone, so a check is needed first.** `"" }}{{ name` gets past the output wrapper, printing nothing with one capture. `name %}{% assign __v = "other"` gets past the assign wrapper. So the design scans for `}}`, `{{`, `%}`, and `{%` outside strings before wrapping. With that scan, neither wrapper accepted any of the 20 attempts, and it wrongly flagged none of the 28 valid Expressions. **Use the scan with the output wrapper,** which is simpler and gives the shorter errors. Parse errors quote the wrapper and count columns from it, so messages should be rewritten to show the author's Expression. As the design guessed, Liquid rejects `{it's} and {don't}` and `{it's}` (`expected Literal`).

**The `attr` filter works.** Through `<Card title="{{ t | attr }}" />`, escaping `&`, `"`, and line endings, all 20,000 random values came back from the Dialect parser exactly, as a block, inline, and in a quote. Without it, 2,923 values came back **silently different** (`&#35;` becomes `#`), 1,551 gave errors, and 938 panicked stock `markdown`. So plain `{{ t }}` in an attribute is worse than the known hazard suggests, and only 14,588 survived.

**YAML to Liquid.** `serde-saphyr` deserializes straight to `liquid::Object`, and it rejects duplicate keys, `.inf`, and `.nan`. Some values change silently, which authors need to know:

| YAML | Becomes | Note |
|---|---|---|
| `1.0`, `1.10` | Prints `1`, `1.1` | Quote version numbers |
| `007` | `7`, typed as fractional | Quote zip codes and IDs |
| `yes`, `on` | `true` | YAML 1.1 behavior |
| `99999999999999999999` | Prints `100000000000000000000` | Over `i64`, it loses precision |
| `2026-01-02` | A date, which prints the same | A full timestamp stays a string |
| Merge keys, `<<: *b` | Error: `data did not match any variant of untagged enum Value` | Anchors alone work. Needs a better message |
| A list or text as the whole frontmatter | Error: `expected mapping start`, with a location | Empty frontmatter is an empty map |

**Scope.** `meta.x` and `x` work together. `{% assign x = "Y" %}` changes `x` but not `meta.x`. `{% assign meta = 1 %}` and `{% for meta in xs %}` silently shadow `meta`, because reserved names are only checked in frontmatter. `{% include %}` and `{% render %}` both give `Partial does not exist`, including for `"/etc/passwd"`, so templates can't read files. Assigned and loop variables don't reach Expressions, and the error names them, so `{% assign y = 2 %}{y}` says `requested variable=y`.

**Whitespace.** These spellings keep a block quote together, as checked by the Dialect parser with the condition true and false:

| Spelling | Result |
|---|---|
| `> a`, `{% if c %}`, `> b`, `{% endif %}`, `> d` | **3 quotes when true** |
| `{%- if c -%}` and `{%- endif -%}` | **Glued: one line reading `a> b> d`** |
| `{%- if c %}` and `{%- endif %}` | **One quote. Recommended** |
| `{% if c -%}` and `{% endif -%}` | One quote |
| `> {% if c %}` and `> {% endif %}` | One quote, but split into paragraphs |

A loop works the same way: `{%- for i in xs %}` makes one quote holding a list. **The tab limit:** the grammar's whitespace is only spaces and line endings, so `a\t{%- if x -%}\tb` keeps its tabs, and `{{\tx }}` is a parse error. Shopify's page says "whitespace" without mentioning tabs, and no Ruby Liquid was available to compare with, so this is a likely divergence rather than a confirmed one. CRLF trims correctly, and Stage 1 normalizes it anyway.

**The PRD's example works.** Its `index.mdx` goes through Liquid and the Dialect parser to exactly the expected tree, its Expressions evaluate to `Q3 Report`, and the Component's `props.kind | upcase` to `ALPHA`. This stock `markdown` result doesn't depend on the fork.

**Error locations:**

| Error | Has a line? | What an author gets |
|---|---|---|
| Liquid syntax, such as `{% if %}` | **Yes**, line and column in the body, so add the frontmatter's length | A good snippet |
| An unclosed `{% if %}` | Only the end of the file, not the opening tag | `Unclosed block. {% endif %} tag expected.` |
| An unknown filter | No | The filter's name |
| An undefined variable or index | No | The name, plus a trace with the loop's position: `from: {% if true %}`, `from: {% for i in xs %}`, `index=3` |
| A filter error, such as dividing by zero | No | `filter=divided_by : 0`, `input="abc"` |
| A bad tag from a loop's third iteration | Dialect line in the rendered text | See below |

**Searching the source for the name in the message** found the right line in all 3 cases with a name in them. With syntax errors' line numbers and filter errors' own text, every one of the 6 cases tested points somewhere usable, though a name used on several lines would give several candidates. The third-iteration case is the one the design worried about, and the rendered snippet does the job:

```text
index.mdx, after the Liquid Pass, line 8: Unexpected character `"` (U+0022) in attribute name, ...
    <Card title="th"ree" />
Line numbers refer to the output of the Liquid Pass.
```

That's a mock of the message we'd write, and the crate only supplies the parts. The source only has `<Card` on line 4, which a search can also report. The hint in this case should be to use `attr`.

**Exact messages for the known hazards** (for writing hints):

| Input | Message |
|---|---|
| `${{ secrets.TOKEN }}` | `Unknown variable`, `requested variable=secrets` |
| `<A style={{color: "red"}} />` | `expected Literal`, at the `:` |
| `<A style={{color}} />`, when `color` exists | Prints `style=1`, then the Dialect says `Unexpected character 1 before attribute value` |
| `{% for i in xs %}<Card title={i.name} />{% endfor %}` | The Liquid Pass succeeds, then `Unknown variable`, `requested variable=i` |
| `{% sales %}` in prose | `Unknown tag`, `requested=sales`, with a location and the tags available |

**Robustness:**

- **Random templates:** 200,000, with 1 caught panic: `{{ n | minus: 9223372036854775807 | round | divided_by: -1 }}`, at `math.rs:330`. So Stage 2 should sit inside the same guard as Stage 3.
- **Integer overflow wraps silently:** `{{ 3 | plus: 9223372036854775807 }}` prints a negative number.
- **Nesting aborts the process on the default stack.** `{% for %}` aborts at 5,000 levels, and `{% if %}`, `{% unless %}`, and `{% capture %}` at 10,000. The abort happens in `parser.parse`, before our code runs anything else, and a guard can't catch it. 2,000 levels are fine for all of them.
- **A larger stack contains it.** On a 1 GiB thread, all four parse at 10,000 levels and at 100,000, though that takes over 40 seconds and `for` then timed out rendering. Nobody writes 2,000 nested blocks, and Liquid output never goes back through the parser, so only the author's own file can do this. A guarded thread with a large stack is enough.
- **Other deep shapes are fine:** long paths and filter chains reached 100,000, and 100,000 nested `[a` gives a caught panic at `parser.rs:60`.
- **Loops are linear and unbounded:** `(1..100000000)` writes 100 MB in 8.3 seconds, which sizes the design's "not a sandbox" note. Memory use wasn't measured.

**Maintenance.** The issues that matter here are old and open: #570 (`default`), [#377](https://github.com/cobalt-org/liquid-rust/issues/377) (whitespace, from 2020). Nothing here suggests anyone's about to change this behavior, which helps a small patch stay small.

**Recommendation: carry the 11-line `liquid-core` patch.** Alternatives are to leave object iteration nondeterministic, breaking R23 for any Project that loops over a map, or to find a way to forbid it. `Object` is also what `{% assign %}` and filters produce, so a custom sorted type for frontmatter wouldn't survive them. `indexmap` would keep YAML order, which authors might prefer, at the cost of a new dependency. It wasn't tested.

**Not tested:** the wider `liquid` test suite with the patch, as above; real Shopify Liquid, for a direct comparison; memory on large loops; Expansion itself, which is Spike 5.

**Worth reporting upstream** (not reported yet): object iteration order, and a sorted map as an option; the `math.rs:330` panic; integer overflow in `plus` and `times`; and a note on #570 that it affects `default` everywhere.

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

**Findings (2026-11-02): Typst is confirmed, with one bar not fully met** ([ADR 0006](adr/0006-typst-is-the-renderer.md)). Numbers are from `spikes/04-typst/results/`, and two runs of the deterministic suites matched exactly. I looked at the rendered structure pages, the script pages, and the landscape, margins, and page numbers pages myself.

| Bar | Result |
|---|---|
| `pdftotext` finds every fixture string, adversarial text unchanged | **Pass.** 1,134 of 1,134 adversarial placements, and 37 of 37 structure strings. Scripts the bundled fonts lack are a separate problem, below |
| SHA-256 identical across two runs | **Pass.** See below |
| SHA-256 identical in a second clean container | **Not met.** No container is possible here, so a chroot on the same machine stands in |
| No system fonts or network | **Pass.** No socket and no font path in `strace`, and output didn't change when fonts were installed |
| Page Settings visibly apply | **Pass.** All 9 settings measured correct, and the 3 I looked at were visibly right. All 7 invalid values were rejected before Typst saw them |

**Determinism.** All of these gave the same SHA-256, for a 2-page mixed document and a 50-page one:

- An empty environment, two other time zones, Japanese and Turkish locales, a missing `HOME`, `SOURCE_DATE_EPOCH`, 1 and 32 Rayon threads, one CPU, and a different working directory.
- 20 separate processes, and 20 runs in one process.
- 14 system font files installed, then removed. This machine started with no fonts, so without this step a pass would mean little.
- A chroot holding only the binary, its 4 libraries, the input, and the images (mixed document only).
- A second clean build in a different path, whose binary differs by about 15 KB, and an LTO build. Both gave the same PDF, which suggests compiler settings don't disturb the layout.

**What this doesn't prove:** everything ran on one x86-64 Linux machine. Typst's maintainers say floating point can make output differ across platforms and CPUs ([#7683](https://github.com/typst/typst/issues/7683)). So whether R23 holds beyond one machine is unknown, and it's in the PRD's open questions. To check, copy the binary and `fixtures/` into a real second container and run `check-determinism.sh`, then compare with `results/record/determinism.txt`.

**Fonts are the main finding:**

| Script | Bundled fonts | With a `fonts/` directory |
|---|---|---|
| Latin, Greek, Cyrillic, Hebrew | Good | Same |
| Arabic | Connected, but stretched in a monospace font | Not tested with a better font |
| Devanagari | No conjuncts, so it reads as separate letters. **Counted as 0 missing glyphs** | Shaped correctly with Lohit, but `pdftotext` repeats a syllable |
| CJK | **Empty boxes** | Correct with IPA Gothic, except Korean, which that font lacks |
| Emoji | **Empty boxes** | Correct, in color. `☑` also turns into an emoji once this font is present |

- **Typst gives no warning for a missing glyph.** Without a check of our own, a Chinese document is a PDF of empty boxes with exit code 0. Walking the laid-out pages counts these reliably, so the Renderer should report them, naming the characters and suggesting `fonts/`.
- **`pdftotext` is a weak test for this.** On the page of boxes, it still extracted most of the text. Only `你` of `你好世界` was lost, so most of the sample would have passed.
- **Fonts from a directory stay deterministic,** with the same hash across two runs.

Bundling more is a size decision: the bundled fonts are about 10 MB, and the IPA Gothic and Noto Color Emoji files alone are 6 MB and 11 MB. The working default in the PRD is to bundle nothing more.

**How to emit Typst.** The design's table used markup, such as `_..._` and `-`, but that depends on neighboring text and line starts. **Use function forms instead:** `#emph[...];`, `#list(...)`, `#heading(level: 2)[...]`, and so on, ending inline calls with `;` so the following text can't become a field access or an argument. Code, URLs, and alt text go through `string_literal`, and all 9 tricky cases compiled with their text intact.

**Escape text as string literals.** Writing text as `#"...";` only needs `\`, `"`, and line endings escaped, so there's no list of Typst syntax to maintain.

| | Escape table | String literal |
|---|---|---|
| Adversarial placements | 567 of 567 | 567 of 567 |
| 20,000 fuzzed strings | 19,999 | **20,000** |
| PDF of the two real documents | **Byte-identical between the two** | |

The table's one miss is a soft hyphen followed by two U+2028 characters, and I didn't isolate exactly why. What I did see separately is that in the table mechanism, `a`, U+2028, `b` prints as `ab`, and two in a row split the paragraph, where a string literal gives line breaks. All text in the fuzz was checked on laid-out pages, with 300 more through Markdown and `pdftotext`, whose 15 misses all contain CJK.

**Files.** Each of these was refused, and the World's log shows nothing outside the root was read: `..`, an absolute path, a symlink leaving the root, a URL, a `file:` URL, and SVGs linking to a file outside, an absolute file, or a URL. `..` and the SVG links never reach our `file()`, because Typst refuses them first. The URL check before compiling is ours, and the symlink check needs our `canonicalize`. Messages are mostly usable, such as `path "../secret.png" would escape the project root`. `![a](/etc/hostname)` looks in the asset root, so it's just not found.

**PDF details.** The title comes from the first heading, there's no creation date, and output is tagged, with bookmarks. `/Creator` says `Typst 0.15.1`.

**Robustness.**

- **Random Markdown:** all 2,000 documents behaved correctly in each mechanism, with 1,972 compiling and 28 correctly reporting a missing image. There were no panics or syntax errors from emitted Typst. That was after fixing two emitter bugs the suite found: a footnote that references itself, and a footnote referenced only from another definition.
- **Nesting gives clean errors:** quotes and lists work at 10 levels, and 50 gives `maximum show rule depth exceeded`, a limit worth a hint. Beyond that, it's `maximum parsing depth exceeded`.
- **One abort, in my emitter:** 10,000 nested emphases aborted before Typst was called, and a 1 GiB stack contained it, so the real emitter needs a depth limit too.

**Measurements, for a person to set targets from:**

| Measure | Result |
|---|---|
| Stripped release binary | 50.0 MB, about 10 MB of it fonts |
| With LTO and one codegen unit | 44.6 MB |
| Clean build | 114 seconds, or 238 for the LTO profile (8 cores) |
| Rendering a 2-page mixed document | 28–46 ms, in a new process each time |
| Rendering 50 pages | 208–233 ms, in a new process each time |
| 50-page PDF | 637 KB |

The spike's "1 page" is 2 here, because the mixed fixture needs 2.

**Vetting.** Upstream's README only documents installing the CLI, so the harness uses git at tag `v0.15.1`. The source of `typst`, `typst-pdf`, and `typst-kit` published on crates.io is identical to that tag. `fontdb` is in the dependency tree through SVG support, but nothing scans for fonts, as `strace` confirms.

**Not tested:** other platforms and CPUs; a better Arabic font; long words, wide tables, and large images overflowing the page; styling, which is Milestone 4.

**Worth reporting upstream** (not reported yet): a warning when a glyph is missing from every font.

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
- Spike 2's untested handling: `<Card title="x"` with no `>` must be an error, a table whose header has an unescaped filter pipe must be an error, and `{x \| upcase}` in a cell must reach Liquid as `x | upcase`.
- `{it's}` must give an error pointing at that Expression, and `{it's} and {don't}` and an Expression containing a blank line must not silently become text.
- `{/* comment */}` must produce nothing, and a generated 256 KB table must finish or fail clearly.
- Spike 3's required handling: `"now" | date` must give an error, `{% for kv in obj %}` must print in the same order in every process, an Expression containing `}}` outside a string must be rejected, and a parse error in an Expression must quote the author's text rather than the wrapper.
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
4. Move Spike 1's suites into the real tests, and start [Milestone 1](technical-design.md#delivery). `spikes/` stays as the evidence for these findings.
