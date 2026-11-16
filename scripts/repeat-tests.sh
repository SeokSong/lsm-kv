#!/usr/bin/env bash
# runs the tests many times in the linux dev container to catch flaky (timing dependent) tests.
# usage: scripts/repeat-tests.sh [runs] [test name filter]
#        scripts/repeat-tests.sh 50 store      50 runs of the tests matching "store"
set -euo pipefail
cd "$(dirname "$0")/.."

runs="${1:-25}"
filter="${2:-}"
docker compose run --rm -T dev bash -c "
  cargo test --no-run -q 2>/dev/null
  failed=0
  for i in \$(seq 1 $runs); do
    if ! out=\$(cargo test -q -- $filter 2>&1); then
      failed=\$((failed + 1))
      echo \"run \$i failed:\"
      echo \"\$out\" | grep -E 'panicked|FAILED' | head -5
    fi
  done
  echo \"\$failed of $runs runs failed\"
  [ \$failed -eq 0 ]
" 2>&1 | grep -v ' Container '
