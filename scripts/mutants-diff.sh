#!/bin/sh
# Mutation-test the Rust lines changed between the merge base with
# ${MUTANTS_BASE:-origin/main} and HEAD. .cargo/mutants.toml limits the scope.
# mutants-run (~/.local/bin) tests a commit: it refuses tracked changes and
# untracked build inputs, sizes the shards to free RAM, pauses them under
# memory pressure, and removes its shard worktrees at the end.
# Exit code 0: every mutant was caught or unviable, or no Rust changed.
# 2: a mutant was missed. 3: a test timed out. 4: the unmutated tree fails.
# 90: critical memory pressure ended the run.
# A test-only diff selects no mutants; rerun the touched file with -f instead.
# Extra arguments go to cargo mutants, for example: -- -- column_widths::
set -eu
cd "$(dirname "$0")/.."
out=$(mktemp -d /private/tmp/gtop-mutants-diff.XXXXXX)
echo "mutants: output in $out"
exec mutants-run diff --base "${MUTANTS_BASE:-origin/main}" -- --output "$out" "$@"
