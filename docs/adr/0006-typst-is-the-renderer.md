---
status: accepted
---

# Typst is the Renderer

Stages 7 and 8 emit Typst source from `pulldown-cmark` events and typeset it in-process with the `typst` and `typst-pdf` crates, version 0.15.1, pinned exactly. [Spike 4](../spikes.md#spike-4-typst-embedding) confirmed this: adversarial text came through unchanged in all 1,134 placements, output was byte-identical under every variation tried, no system font or network call was used, and Page Settings applied. It needs no browser, so it meets R21, and it renders 50 pages in about 0.2 seconds. Fulgur, the named fallback, wasn't needed. The costs are a 50 MB binary, a Typst API that changes between releases, and fonts: the bundled ones have no CJK or emoji.

**Byte-identical output is only proven on one platform.** The spike's checks all ran on one x86-64 Linux machine, because no second container was available. Typst's maintainers have said that floating point can make output differ across platforms and CPUs ([#7683](https://github.com/typst/typst/issues/7683)). So R23 holds for the same binary on this machine, and a wider claim still needs testing with `spikes/04-typst/check-determinism.sh`.

**The dependency comes from upstream's tag.** Upstream's README only documents installing the CLI, so the crates come from git at `v0.15.1`. The source of `typst`, `typst-pdf`, and `typst-kit` published on crates.io is identical to that tag.

## Considered options

- **Fulgur:** the named fallback, emitting HTML. Created in 2026-03, its README says API stability isn't guaranteed and that output can vary across machines. Since Typst passed, it wasn't tested.
- **A browser, through `chromiumoxide` or `headless_chrome`:** needs a browser at runtime, which R21 rules out.
- **`cmarker`:** a Typst package that reads Markdown. It adds a second parser, and its raw-Typst comments conflict with R13.

Because a document can only ever contain Markdown ([ADR 0002](0002-components-only-produce-markdown.md)), replacing the Renderer later would only affect stages 7 and 8, which makes this easier to reverse than most choices here.
