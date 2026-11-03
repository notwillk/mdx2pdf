#!/usr/bin/env bash
# Spike 5: builds the patched markdown and liquid-core that Cargo.toml points at, as
# ADR 0004 and ADR 0005 describe. vendor/ is ignored by git. Works offline if Cargo has
# fetched these already, for example by building spikes/01-serializer and spikes/03-liquid.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
spikes=$(dirname "$here")
cargo_home=${CARGO_HOME:-$HOME/.cargo}
rm -rf "$here/vendor"
mkdir -p "$here/vendor"

# markdown: upstream's 1506572 plus six patches, in this order.
checkout=$(ls -d "$cargo_home"/git/checkouts/markdown-rs-*/1506572 2>/dev/null | head -1 || true)
if [ -n "$checkout" ]; then
  cp -r "$checkout" "$here/vendor/markdown"
  chmod -R u+w "$here/vendor/markdown"
else
  git clone https://github.com/wooorm/markdown-rs "$here/vendor/markdown"
  git -C "$here/vendor/markdown" checkout 1506572f9b406431402928f3a8b3df0b4ae2d8f5
fi
rm -rf "$here/vendor/markdown/.git"
for pr in 265 267 219 210 205 207; do
  patch -d "$here/vendor/markdown" -p1 --no-backup-if-mismatch -s < "$spikes/02-dialect/results/fork/pr$pr.patch"
done

# liquid-core: the published 0.26.11, with its object map sorted.
published=$(ls -d "$cargo_home"/registry/src/*/liquid-core-0.26.11 2>/dev/null | head -1 || true)
if [ -z "$published" ]; then
  echo "liquid-core 0.26.11 isn't in Cargo's cache. Build spikes/03-liquid first." >&2
  exit 1
fi
cp -r "$published" "$here/vendor/liquid-core"
chmod -R u+w "$here/vendor/liquid-core"
patch -d "$here/vendor/liquid-core" -p1 --no-backup-if-mismatch -s < "$spikes/03-liquid/results/object-order/btreemap.patch"

# pulldown-cmark-to-cmark: the published 23.0.0, whose validator needs to know about footnotes.
published=$(ls -d "$cargo_home"/registry/src/*/pulldown-cmark-to-cmark-23.0.0 2>/dev/null | head -1 || true)
if [ -z "$published" ]; then
  echo "pulldown-cmark-to-cmark 23.0.0 isn't in Cargo's cache. Build spikes/01-serializer first." >&2
  exit 1
fi
cp -r "$published" "$here/vendor/pulldown-cmark-to-cmark"
chmod -R u+w "$here/vendor/pulldown-cmark-to-cmark"
patch -d "$here/vendor/pulldown-cmark-to-cmark" -p1 --no-backup-if-mismatch -s < "$here/patches/footnote-validation.patch"

echo "vendor/markdown, vendor/liquid-core, and vendor/pulldown-cmark-to-cmark are ready"
