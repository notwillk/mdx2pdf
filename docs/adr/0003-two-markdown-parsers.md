---
status: accepted
---

# Two Markdown parsers: `markdown` for the Dialect, `pulldown-cmark` for Markdown

Stage 3 parses the Dialect with `markdown`, and Stage 6 parses Intermediate Markdown with `pulldown-cmark`. `markdown` is the only Rust parser for MDX syntax we found, but [Spike 1b](../spikes.md#spike-1b-event-based-serializer) showed it panics on an ordinary typo, an unclosed code fence in a list item followed by a different kind of list, and its repo has had no commits since April 2025. `pulldown-cmark` had no panics on 100,000 random documents, and read our serialized output correctly for all 624 spec inputs on the production path. So a plain `.md` file never touches `markdown`, and that parser stays confined to the one job nothing else does, inside a panic guard. The cost is two parsers with different data shapes, which an adapter bridges, and two grammars that could disagree, which the round-trip check catches.

## Considered options

- **`markdown` for both:** one tree type and one parser, which is what the design started with. But it exposes plain Markdown input to the panics, and the Dialect and Markdown grammars differ anyway, so one parser wouldn't have meant one grammar.
- **Our own MDX tag scanner in front of `pulldown-cmark`:** would remove `markdown` entirely, as executable.md's own scanner suggests is possible, but it's a larger job than a guard around the existing parser. Worth revisiting if Spike 2 shows the guard isn't enough.
