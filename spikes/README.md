# Spikes

Throwaway experiments behind [docs/spikes.md](../docs/spikes.md), kept so the findings can be checked and reproduced. Nothing here is production code.

| Directory | Spike | Reproduce |
|---|---|---|
| `01-serializer/` | 1 and 1b | `cargo run --release --bin spike-serializer -- <label>`, or `--bin events -- <label>` |
| `02-dialect/` | 2 | `cargo run --release -- run <label>`, or `deep <label>` for depth and scaling |
| `04-typst/` | 4 | `cargo run --release -- run <label>`, or `deep <label>` for nesting and timing. `check-determinism.sh <binary> fixtures <scratch dir>` compares hashes, including in a second container. Set `SPIKE_EXTRA_FONTS` to a directory to test fonts from there. Needs `pdftotext` |
| `03-liquid/` | 3 | `cargo run --release -- run <label>`, or `deep <label>` for depth, loops, and object order. Apply `results/object-order/btreemap.patch` to a copy of `liquid-core` 0.26.11 to get the patched run |
| `05-expansion/` | 5 | Run `setup-vendor.sh`, which patches copies of its three dependencies. Then `cargo run --release -- run <label>`, and `render <label> <spike-typst binary>`, which needs `pdftotext`. `one <directory>` expands a single Project |

Each writes to `results/<label>/`.

**Before running Spike 2, 3, or 4, turn off core dumps with `ulimit -c 0`.** Their controls abort or overflow on purpose, and a dump holds process memory, including environment variables and any tokens in them. `.gitignore` here keeps dumps out of commits, but it's better not to create them.

## Third-party content

`01-serializer/spec.json` is the [CommonMark 0.31.2 spec's](https://spec.commonmark.org/0.31.2/) list of examples, unchanged, and result files quote those examples. The spec is Copyright (C) 2014-16 John MacFarlane, released under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/), not this repository's MIT license.

`02-dialect/results/fork/` holds patches from open pull requests on [wooorm/markdown-rs](https://github.com/wooorm/markdown-rs), which is MIT licensed.
