# gtop agent rules

`CLAUDE.md` is a symlink to this file.

## Checks

Run from the repository root:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Mutation tests

- `.cargo/mutants.toml` lists the domain files that cargo-mutants mutates. GPUI views, FFI shells, and test code stay out.
- Before a push that changes domain Rust code, run `sh scripts/mutants-diff.sh`. It mutates only the changed lines. It runs with `--jobs 2` and stops after 15 minutes. Report the untested mutants when it stops at the cap.
- A missed mutant needs a behavior test, or a refactor that makes the code testable. An equivalent mutant goes into `exclude_re` with a reason comment.
- For one file, run `cargo mutants --no-config -p <crate> -f <file> -j 2 --exclude-re 'signal_target_is_safe|kill_process' --timeout-multiplier 3 --minimum-test-timeout 60 -- -- <module>::`. Use `--no-config`, because `-f` adds to `examine_globs` instead of narrowing it. `--no-config` also drops the config's `exclude_re`, so keep the `--exclude-re` for the kill path. Stop the run after 30 minutes.
- Never mutate the kill path (`signal_target_is_safe`, `ProcessMonitor::kill_process`, `AppState::kill_process*`). It sends real signals, and a mutant could reach pid <= 1. In another repo such a mutant sent `kill(-1, SIGKILL)` and killed every user process.
- The machine is shared: run one mutation process at a time, with `--jobs` at most 2. Run a full sweep only at night or when the owner asks.

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
