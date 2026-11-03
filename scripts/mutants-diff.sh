#!/bin/sh
# Mutation-test the Rust lines changed since the merge base with origin/main,
# including uncommitted changes. .cargo/mutants.toml limits the scope.
# Exit code 0: every mutant was caught or unviable. 2: a mutant was missed.
# 3: a test timed out. 4: the unmutated tree fails its tests.
# Extra arguments go to cargo mutants, for example: -- column_widths::
set -eu
cd "$(dirname "$0")/.."
base=$(git merge-base "${MUTANTS_BASE:-origin/main}" HEAD)
diff_file=$(mktemp)
trap 'rm -f "$diff_file"' EXIT
git diff "$base" -- '*.rs' >"$diff_file"
if [ ! -s "$diff_file" ]; then
  echo "mutants: no Rust changes since $base"
  exit 0
fi
cargo mutants --in-diff "$diff_file" --jobs "${MUTANTS_JOBS:-2}" \
  --output "${MUTANTS_OUT:-target/mutants-diff}" "$@"
