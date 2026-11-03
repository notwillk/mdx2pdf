---
status: accepted
---

# Patch `pulldown-cmark-to-cmark` to validate footnotes

We carry a small patch to `pulldown-cmark-to-cmark` 23.0.0, with 21 lines added and 1 removed in `src/render.rs`. Before writing a block with emphasis, strong, or strikethrough, the stock crate checks its output by parsing that block alone. A footnote's definition is a block of its own, so a reference in the block reads as text, the check fails, and the crate returns `Unrepresentable`. So it refuses any paragraph with both emphasis and a footnote, such as `*w* x[^1]`, even in a document with no Components. [Spike 5](../spikes.md#spike-5-end-to-end-expansion) saw 30,473 of 100,000 valid documents refused this way, and with the patch, 19 were refused in all. The patch supplies a definition for each reference during that check only, so it doesn't change what the crate writes. The only other change in Spike 1b's suites was clearing 12 more of these refusals. The cost is a third patched dependency, on a crate that's moving fast, so we have to re-apply it on any upgrade. A Markdown Input is unaffected, because it never reaches Stage 5.

**Build it from the saved patch.** Apply `spikes/05-expansion/patches/footnote-validation.patch` to the published `pulldown-cmark-to-cmark` 23.0.0 source, and point `[patch.crates-io]` at the result. That's what Spike 5 tested, and `spikes/05-expansion/setup-vendor.sh` does it. The patch's paths are relative to that crate's root.

**Still to do:** run the crate's own tests with the patch, which Spike 5 didn't, and report the bug upstream. The upstream project is active, so drop the patch if a release fixes this.

## Considered options

- **Placeholder text for footnote references:** write each reference as text before serializing, and restore it afterward. It needs no patch, but it's a hack, and it wasn't tested.
- **Living with the refusal:** no patch, but a paragraph with bold text and a footnote is an ordinary document, and it would be an error.
- **Our own serializer:** removes the dependency, but it means owning roughly 600–900 lines of escaping and delimiter logic to fix one check. It stays the fallback if this crate becomes a problem.
