#!/usr/bin/env bash
# Runs on every container start, so it must be safe to run repeatedly.
set -euo pipefail

min_minor=92

export PATH="$HOME/.cargo/bin:$PATH"

if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain stable
fi

version="$(rustc --version | cut -d' ' -f2)"
major="$(echo "$version" | cut -d. -f1)"
minor="$(echo "$version" | cut -d. -f2)"
if [ "$major" -lt 1 ] || { [ "$major" -eq 1 ] && [ "$minor" -lt "$min_minor" ]; }; then
  echo "install.sh: Rust $version is older than the required 1.$min_minor. Run: rustup update stable" >&2
  exit 1
fi

echo "install.sh: Rust $version ready"

# Test-only: the renderer's tests and Spike 4 check PDFs with pdftotext and friends.
if ! command -v pdftotext >/dev/null 2>&1; then
  sudo apt-get install -y -qq poppler-utils || echo "install.sh: couldn't install poppler-utils, so PDF checks won't run" >&2
fi
