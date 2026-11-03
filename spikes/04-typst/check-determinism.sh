#!/usr/bin/env bash
# Spike 4: prints a SHA-256 for each rendering, under conditions that shouldn't matter.
# Usage: check-determinism.sh <spike-typst binary> <fixtures dir> <scratch dir>
# Copy the binary and fixtures into a second clean container, run this there, and compare
# the summary lines with results/record/determinism.txt.
set -euo pipefail
ulimit -c 0

bin=$(realpath "$1")
fixtures=$(realpath "$2")
work=$(realpath "$3")
assets="$fixtures/assets"
mkdir -p "$work"
"$bin" dump "$work"

render() { # name, then a command prefix
  local name=$1 file=$2
  shift 2
  "$@" "$bin" render "$work/$file.md" "$work/out-$name.pdf" --assets "$assets"
  sha256sum "$work/out-$name.pdf" | cut -d' ' -f1
}

for file in structure long; do
  echo "== $file"
  base=$(render "$file-base" "$file" env)
  echo "baseline           $base"
  check() {
    local label=$1
    shift
    local got
    got=$(render "$file-$label" "$file" "$@")
    if [ "$got" = "$base" ]; then echo "same               $label"; else echo "DIFFERENT $got $label"; fi
  }
  check empty-env env -i
  check tz-tokyo env TZ=Asia/Tokyo
  check tz-honolulu env TZ=Pacific/Honolulu
  check lang-ja env LANG=ja_JP.UTF-8 LC_ALL=ja_JP.UTF-8
  check lang-tr env LANG=tr_TR.UTF-8 LC_ALL=tr_TR.UTF-8
  check home-missing env HOME=/nonexistent
  check source-date-epoch env SOURCE_DATE_EPOCH=1000000000
  check threads-1 env RAYON_NUM_THREADS=1
  check threads-32 env RAYON_NUM_THREADS=32
  command -v taskset >/dev/null && check one-cpu taskset -c 0
  got=$(cd / && "$bin" render "$work/$file.md" "$work/out-cwd.pdf" --assets "$assets" && sha256sum "$work/out-cwd.pdf" | cut -d' ' -f1)
  if [ "$got" = "$base" ]; then echo "same               cwd-root"; else echo "DIFFERENT $got cwd-root"; fi
  distinct=$(for i in $(seq 1 20); do "$bin" render "$work/$file.md" "$work/out-rep.pdf" --assets "$assets"; sha256sum "$work/out-rep.pdf" | cut -d' ' -f1; done | sort -u | wc -l)
  echo "$distinct distinct result(s) in 20 separate processes"
done
"$bin" repeat "$work/structure.md" "$assets"
"$bin" render "$work/structure.md" "$work/out-page.pdf" --assets "$assets" --page "$work/page.toml"
echo "with page settings $(sha256sum "$work/out-page.pdf" | cut -d' ' -f1)"
