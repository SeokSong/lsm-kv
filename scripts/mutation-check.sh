#!/usr/bin/env bash
# checks that tests can fail: breaks one line of code on purpose (a sed expression), runs the
# tests, and puts the file back exactly as it was, even if the run is interrupted.
# passes when at least one test FAILS with the broken code.
#
# usage: scripts/mutation-check.sh <file> <sed expression> [test name filter]
#        scripts/mutation-check.sh crates/lsm-kv/src/store.rs 's/Op::Delete => {}/Op::Delete => results.push(KeyValue { key: record.key, value: Vec::new() }),/' scan
set -euo pipefail
cd "$(dirname "$0")/.."

file="$1"
expression="$2"
filter="${3:-}"

backup="$(mktemp)"
cp "$file" "$backup"
trap 'cp "$backup" "$file"; rm -f "$backup"' EXIT

sed -i.sed-tmp "$expression" "$file" && rm -f "$file.sed-tmp"
if cmp -s "$file" "$backup"; then
  echo "the expression changed nothing in $file, nothing was tested"
  exit 2
fi
echo "mutated: $(diff "$backup" "$file" | grep '^>' | head -3)"

if docker compose run --rm -T dev cargo test --workspace -q -- "$filter" >/dev/null 2>&1; then
  echo "NOT CAUGHT: every test still passes with the broken code"
  exit 1
fi
echo "caught: at least one test fails with the broken code"
