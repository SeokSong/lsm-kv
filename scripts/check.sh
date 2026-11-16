#!/usr/bin/env bash
# everything CI checks, run in the linux dev container: fmt, clippy, tests, benchmarks build.
# usage: scripts/check.sh            all checks
#        scripts/check.sh merge      only tests whose name contains "merge" (fmt + clippy still run)
set -euo pipefail
cd "$(dirname "$0")/.."

filter="${1:-}"
docker compose run --rm -T dev bash -c "
  set -euo pipefail
  cargo fmt --check
  cargo clippy --all-targets -- -D warnings
  cargo test -- $filter
  cargo bench --no-run
" 2>&1 | grep -vE '^ *Container |^ *Compiling |^ *Executable '
