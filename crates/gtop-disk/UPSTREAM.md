# Upstream source and local changes

Source: [tobi/disktree](https://github.com/tobi/disktree).
Commit: `9b18b272ca3c5a145f6ff58eaf89dc4d311eeaea`.
Original location: `crates/disktree-core/src`.
The complete upstream MIT license is in [LICENSE](LICENSE).
Copyright (c) 2026 Tobi Lütke.

## Copied files

The import contains `access.rs`, `classify.rs`, `filter.rs`, `identity.rs`,
`insights.rs`, `macos.rs`, `removal.rs`, `scan.rs`, `scan_preview.rs`,
`scan_threads.rs`, `size.rs`, `space.rs`, `tree.rs`, and `treemap.rs`.
`scan_preview` is required by `ScanProgress`.

## Excluded code

The import excludes `deployment`, `export`, `payload`, `remote`,
`remote_progress`, `remote_walk`, `windows`, and `mft`.
Windows-only items and scan blocks are removed from the copied files.
Shared pure platform comparison helpers remain where the Unix code uses them.
The supported filesystem backend is Unix, with macOS bulk enumeration.
There is no UI dependency.

All upstream removal executors are removed, including permanent deletion,
macOS system Trash, XDG Trash, external commands, and Windows Recycle Bin.
The retained removal module only plans targets. A plan grants no authority
to execute a move. Gtop retains its descriptor-relative guarded Trash
executor and all its tests. Cleanup stays inside home and on one volume.
No API empties Trash.

Windows administrator checks and privilege restart functions are removed.
`access` retains the tri-valued Full Disk Access query.

## Local changes

- The package is `gtop-disk`, version 1.2.0, with Rust 1.99.1.
  Edition 2024 is retained because upstream uses let chains. Gtop's other
  crates use edition 2021. The workspace has no shared lint configuration.
  Standard workspace clippy checks apply to this crate.
- `lib.rs` declares only the selected modules and the local contract tests.
- The manifest includes only dependencies required by the retained graph.
  CPU sampling uses gtop's existing sysinfo 0.39. The import adds no second
  sysinfo version. No new UI or removal dependency is present.
- Default scan admission retains upstream's full worker pool. The adaptive
  governor remains opt-in and its CPU limit defaults to None. Pool workers
  explicitly use normal macOS QoS.
- `Node` has an optional exact `OsStr` name beside its display name.
  Scanner leaf results and completed directories retain that exact name.
  `path_of` uses the exact name. Synthetic nodes fall back to their text name.
  A local test covers two invalid UTF-8 names with identical display text.
  That test constructs nodes because APFS rejects invalid UTF-8 file creation.
  A separate filesystem test checks exact directory and file names after a scan.
  The node-size test changes from 88 to 136 bytes on 64-bit targets.
- Directory aggregation saturates file and directory counts.
- Removal planning tests are retained from the upstream planning section.
  Executor tests are excluded with the removed executors. Gtop's existing
  guarded removal tests remain unchanged.
- Orphaned documentation for removed items is deleted. The mount-identity
  test for the removed executor's `Volume` helper is excluded. The adaptive
  controller test explicitly sets its CPU ceiling after the default changes.
- The app uses this crate for production folder scans and the headless scan flag.
  The independent serial accounting fixture remains in the app tests.
  That fixture compares the production scanner with the independent serial
  oracle, including hardlinks, a directory
  symlink, sparse data, and allocated directory blocks.
- Formatting follows the destination workspace's `cargo fmt` output.
- Mutation scope includes the imported domain files. `macos.rs` is
  excluded because it is the bulk enumeration FFI shell. Test files stay
  excluded.

## Scanner contract adaptations

The walker pins the root descriptor. Each directory job reopens its ancestor
chain with descriptor-relative no-follow opens. It compares the listed device
and inode before and after each open. Jobs retain no directory descriptors
while queued. Enumeration uses one descriptor per active worker; reopening
uses at most two, in addition to the scan's root descriptor.

Directory and leaf nodes retain observed device and inode. Hardlink accounting
uses a separate flag, so identity retention does not require a deduplication
entry for every ordinary file. Directory allocations remain separate from
derived subtree bytes. Aggregation includes those allocations and unreadable
counts. Reaggregation does not count an error or directory allocation twice.

Cloud entries retain their names, identities, cloud state, and observed local
blocks. The walker does not schedule their contents. Symlinks remain leaves.
The old `follow_links` option now rejects true. A skipped volume or excluded
alias remains visible with zero charged bytes and incomplete coverage.
Cancellation, depth truncation, unreadable contents, and resource exhaustion
cannot produce complete coverage. The walk has a hard depth ceiling of 256.

`max_retained_bytes` defaults to 256 MiB of conservative reservations. Each
entry reserves 1024 bytes plus eight times its full path length. The charge
covers node capacity, name storage, pending tasks, ancestry, and hardlink set
overhead. Cached subtrees and completed-subtree clones reserve the same amount.
Reservations never decrease. Exhaustion stops enumeration and returns partial
data with `limited` set. This is an allocation budget, not a hard RSS limit.
The deduplication bitmap adds at most 8 MiB. Enumeration buffers add 64 KiB per
active worker. Rayon stacks and allocator overhead also contribute to RSS.

Progress retains at most one completed top-level subtree for consumption.
Publication uses real filesystem nodes with identities and derived totals.
Optional publication does not exhaust the scan budget. Clones are admitted
only while total reservations remain below half of the configured budget.
Repeated consumption does not reset those reservations.

Default scans retain the full disktree worker pool with no adaptive admission.
Pool workers explicitly use normal macOS QoS. Explicit fixed-size pools use
the same priority. The scanner does not request Utility QoS.

`examples/scan_contract.rs` is a headless release probe. It prints elapsed
time, output counts, unreadable count, coverage, cancellation, resource-limit
state, reservations, and worker count. External `/usr/bin/time -l` measures
peak RSS.

## Production migration gate

The folder view and headless flag use this scanner. The app retains its bounded
folder projection, command coordinator, and guarded Trash executor. The retired
recursive scanner is removed in a separate commit.

The shared fixture compares allocation, item count, unreadable count, and root
identity with the independent serial oracle. Full-workload resource acceptance
and five alternating old/new scans remain with the coordinator. Fixture parity
does not establish a resource budget. No treemap UI ships here.

## Production adapter and review fixes

The caller can pass a root descriptor opened through a checked scope. The
scanner compares the path observation with that descriptor and never reopens
the root by pathname. Depth truncation records an unreadable directory.

Completed snapshots finish outside the publication mutex. Nested Rayon work
can publish another subtree while a snapshot finishes. The one-slot consumer
retains the first completed snapshot. Every clone is charged before finishing.

Known subtrees retain their defining scan options. Measurement, hidden-entry,
volume, hardlink, link-following, and metric changes reject reuse. Finite depth
scans reject reuse because depth is relative to the scan root.
