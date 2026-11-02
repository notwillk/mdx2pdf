---
status: accepted
---

# Carry a small fork of `markdown`

Stage 3 uses our own fork of `markdown`: upstream's commit `1506572` (version 1.0.0) plus six open upstream PRs, pinned by commit. [Spike 2](../spikes.md#spike-2-dialect-parsing) showed that stock 1.0.0 panics on ordinary typing, such as `<Card title="AT&T" />` and an unclosed tag followed by `---`. A guard catches those panics, but the error can't say where the problem is. Upstream has had no commits since April 2025, so fixes that exist only as open PRs won't reach a release we can depend on. The six PRs are 91 lines added and 23 removed, and they took panics from 1,591 to 5 in 250,000 random documents, and malformed trees from 15 to 0, without changing any of the 652 spec comparisons. The cost is that we own a dependency: we pick up any other upstream fixes by hand, and we have to move back if upstream resumes.

The guard and the tree validator stay. The fork still panics on a rare shape, a table header followed by a list containing an unclosed `<`, and it may have other bugs we haven't found.

| PR | Commit | Fixes |
|---|---|---|
| [#265](https://github.com/wooorm/markdown-rs/pull/265) | `e26c525bd7` | An `&` followed by letters at the end of an attribute |
| [#267](https://github.com/wooorm/markdown-rs/pull/267) | `ccc6ed01ea` | A list after an item ending in unclosed code |
| [#219](https://github.com/wooorm/markdown-rs/pull/219) | `a1129c332d` | A setext underline after an unclosed tag |
| [#210](https://github.com/wooorm/markdown-rs/pull/210) | `79d3a45e94` | Adjacent setext headings |
| [#205](https://github.com/wooorm/markdown-rs/pull/205) | `44542f924d` | List item positions around code |
| [#207](https://github.com/wooorm/markdown-rs/pull/207) | `d2efc4814c` | Whitespace in link titles and labels |

**Still to do:** create the fork and run its own test suite, which Spike 2 couldn't run. If that suite shows a regression from one of these PRs, drop or fix that PR, and record it here.

## Considered options

- **Stock 1.0.0 with the guard:** no dependency to own, and every panic was caught. But ordinary mistakes would get an error with no location, and malformed trees would have to be rejected instead of parsed correctly.
- **Our own MDX tag scanner in front of `pulldown-cmark`:** removes `markdown` entirely, but it's a much larger job than six small patches, and the spike found the parser's trees correct once these bugs are fixed.
- **Vendoring the source into this repo:** same code, but it would add about 42,000 lines to the repo for changes that fit in a fork's small diff.
