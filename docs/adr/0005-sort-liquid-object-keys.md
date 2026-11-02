---
status: accepted
---

# Patch `liquid-core` to sort object keys

We carry an 11-line patch to `liquid-core` 0.26.11 that makes `Object` a `BTreeMap` rather than a `HashMap`. Stock `Object` uses Rust's randomly seeded hasher, so [Spike 3](../spikes.md#spike-3-liquid-behavior) saw `{% for kv in obj %}` print 6 different orders in 6 runs on the same input, which breaks R23, and `liquid` has no option to change that. With the patch, the order was the same in all 6 runs, and the only other difference across the spike's suites was a key list in one error message, now sorted. Keys come out alphabetically rather than in YAML order, so authors who need an order should use a list. The cost is that we own a patch on a dependency, and have to re-apply it on any `liquid` upgrade, though `liquid`'s last release was in February 2025 and the change is a type swap in one file.

**Build it from the saved patch.** Apply `spikes/03-liquid/results/object-order/btreemap.patch` to the published `liquid-core` 0.26.11 source, and point `[patch.crates-io]` at the result. That's what Spike 3 tested. The patch's paths are relative to that crate's root. `liquid` itself stays the published crate.

**Still to do:** run the wider `liquid` workspace's tests with the patch, which hold the conformance suite. Spike 3 couldn't, and only `liquid-core`'s own 121 tests have passed with it. If that suite shows a regression that matters, fix the patch and record it here.

## Considered options

- **Leave iteration random:** no patch, but any Project that loops over a map or prints one would break R23, with output that varies between runs and is hard to explain.
- **A custom sorted type for frontmatter:** `liquid` accepts custom value types, but `{% assign %}` and filters convert values back to `Object`, so the order would be lost as soon as a template used them.
- **Forbid iterating objects:** we found no way to do this short of replacing the `for` block, and `{{ obj }}` prints in the same order anyway.
- **`indexmap`, to keep YAML order:** authors might prefer it, but it adds a dependency and a larger change, and it wasn't tested. Worth revisiting if alphabetical order proves surprising.
- **Forking `liquid` in full:** unnecessary for a change to one file in one crate.
