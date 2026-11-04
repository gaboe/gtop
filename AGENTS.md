# gtop agent rules

`CLAUDE.md` is a symlink to this file.

## Checks

Run from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

On the host, also run `cargo test -p gtop-core kernel_table_and_pid_lookup_agree_on_live_and_zombie_identity -- --ignored`.

## Mutation tests

- `.cargo/mutants.toml` lists the domain files that cargo-mutants mutates. GPUI views, FFI shells, and test code stay out.
- Never run plain `cargo mutants`. Every run goes through `mutants-run` (`~/.local/bin`). It sizes the shards to free RAM, pauses them under memory pressure, and removes its shard worktrees. Do not pass `--jobs`, `-j`, `--in-place`, `--shard`, or a `CARGO_TARGET_DIR`.
- Before a push that changes domain Rust code, commit, then run `sh scripts/mutants-diff.sh`. It mutates only the lines changed between the merge base with `origin/main` and `HEAD`. Uncommitted changes make it refuse to start.
- A missed mutant needs a behavior test, or a refactor that makes the code testable. An equivalent mutant goes into `exclude_re` with a reason comment.
- For one file, run `mutants-run sweep -- --no-config -p <crate> -f <file> --exclude-re 'signal_target_is_safe|kill_process|tests::|_tests\.rs:|/tests/' --build-timeout-multiplier 4 --timeout-multiplier 3 --minimum-test-timeout 60 --output ~/.cache/gtop-mutants/<name> -- -- <module>::`. Use `--no-config`, because `-f` adds to `examine_globs` instead of narrowing it. `--no-config` also drops the config's `exclude_re`, so keep the `--exclude-re` for the kill path and test code. Resume an interrupted run with `--iterate` on the same output.
- Never mutate the kill path (`signal_target_is_safe`, `ProcessMonitor::kill_process`, `AppState::kill_process*`). It sends real signals, and a mutant could reach pid <= 1. In another repo such a mutant sent `kill(-1, SIGKILL)` and killed every user process.
- Never mutate test code (`tests::` modules, `*_tests.rs`, `tests/*.rs`). cargo-mutants mutates test helpers too, and a mutated helper can feed pid -1 to a kill. Test kills use a std `Child` handle only; a kill with a numeric pid in a test needs a `pid > 1` check.
- The machine is shared. A full sweep waits for its turn in the sweep queue (coordinator gsource t12) and runs in its own worker and worktree. The diff gate needs no turn.

## Workers and reviewers

- A worker writes the whole slice first. Then it reads its own diff and runs the checks once, on the clean final commit.
- After a failed check, the worker fixes the cause with narrow tools, such as one filtered test or clippy. Then it runs the checks once more.
- Before the final checks, a worker runs only a red test that its brief requires.
- A worker writes its receipts in `result.md`: commands, exit codes, and log paths.
- Builds run normally. Do not pause builds and do not run unlimited builds.
- Reviewers and verifiers are read-only. They read the diff and the worker receipts and do not run the checks again.
- A reviewer looks for what checks cannot see: behavior, missing cases, design, and tests that pass on a defect. A missing receipt is a finding.
- Put the worker rules in every worker brief and the reviewer rules in every reviewer brief.

## Screen

- Never draw on the user's displays. No full-screen covers, and no visible, activated, moved, hidden, or captured windows. The user has one monitor and works on it.
- Measure with headless GPUI test contexts, the headless draw benchmark, the telemetry of the installed app (`perf.sqlite` and `crates/gtop/scripts/perf-report.py`), or passive `proc_pid_rusage` of the running gtop.
- Put this rule in every brief.

## Dependencies

- Stay on `gpui-pre` 0.3.9 and `gpui-component` 0.8.0.
- Do not add experimental dependencies, such as the `gpui-fast` backend, even for speed. Look for speed in gtop's own code first.

## Delivery

- Do not open pull requests. After the review passes, push straight to `main` as a normal fast-forward: `git push origin <sha>:main`. Rebase on `origin/main` first when necessary. Never force-push.
- After each finished change (committed, checks pass), build and install the app without asking:
  1. Back up `~/Library/Application Support/gtop` when the change touches stored data.
  2. Run `sh crates/gtop/package-macos.sh`.
  3. Quit gtop with `osascript -e 'quit app "gtop"'`. Never kill it.
  4. Replace the bundle: `ditto target/gtop-native/gtop.app /Applications/gtop.app`.
  5. Open it again: `open -a /Applications/gtop.app`.
