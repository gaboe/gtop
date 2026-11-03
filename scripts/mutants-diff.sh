#!/bin/sh
# Mutation-test the Rust lines changed since the merge base with origin/main,
# including uncommitted changes. .cargo/mutants.toml limits the scope.
# Shared-Mac budget: --jobs 2 and a 15-minute wall cap. At the cap cargo-mutants
# stops and keeps its results; report the untested mutants as a gap.
# Exit code 0: every mutant was caught or unviable. 2: a mutant was missed.
# 3: a test timed out. 4: the unmutated tree fails its tests. 124/130: the cap hit.
# A test-only diff selects no mutants; rerun the touched file with -f instead.
# Extra arguments go to cargo mutants, for example: -- -- column_widths::
set -eu
export TMPDIR=/private/tmp
cd "$(dirname "$0")/.."
base=$(git merge-base "${MUTANTS_BASE:-origin/main}" HEAD)
diff_file=$(mktemp /private/tmp/mutants-diff.XXXXXX)
trap 'rm -f "$diff_file"' EXIT
git diff "$base" -- '*.rs' >"$diff_file"
if [ ! -s "$diff_file" ]; then
  echo "mutants: no Rust changes since $base"
  exit 0
fi
out=$(mktemp -d /private/tmp/gtop-mutants-diff.XXXXXX)
echo "mutants: output in $out"
timeout -s INT "${MUTANTS_CAP_SECONDS:-900}" \
  cargo mutants --in-diff "$diff_file" --jobs "${MUTANTS_JOBS:-2}" --output "$out" "$@"
