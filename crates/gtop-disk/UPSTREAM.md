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
  governor remains opt-in and its CPU limit defaults to None. No QoS
  adjustment is added.
- `Node` has an optional exact `OsStr` name beside its display name.
  Scanner leaf results and completed directories retain that exact name.
  `path_of` uses the exact name. Synthetic nodes fall back to their text name.
  A local test covers two invalid UTF-8 names with identical display text.
  That test constructs nodes because APFS rejects invalid UTF-8 file creation.
  A separate filesystem test checks exact directory and file names after a scan.
  The node-size test changes from 88 to 104 bytes on 64-bit targets.
- Directory aggregation saturates file and directory counts.
- Removal planning tests are retained from the upstream planning section.
  Executor tests are excluded with the removed executors. Gtop's existing
  guarded removal tests remain unchanged.
- Orphaned documentation for removed items is deleted. The mount-identity
  test for the removed executor's `Volume` helper is excluded. The adaptive
  controller test explicitly sets its CPU ceiling after the default changes.
- The app has a test-only dependency for a shared accounting fixture.
  That fixture compares the production scanner, the independent serial
  oracle, and the imported scanner, including hardlinks, a directory
  symlink, sparse data, and allocated directory blocks.
- Formatting follows the destination workspace's `cargo fmt` output.
- Mutation scope includes the imported domain files. `macos.rs` is
  excluded because it is the bulk enumeration FFI shell. Test files stay
  excluded.

## Scanner migration gate

The production scanner is unchanged. This crate is isolated from app code
except for tests. No treemap UI ships in this slice.

The imported scanner omits directory allocations. The accounting fixture
compares actual directory blocks, which can be zero on APFS.
The fixture expects the production scanner and oracle to agree. It expects
their total to equal the imported total plus observed directory blocks.
It also records that the imported root has no filesystem identity.
Matching bytes alone does not establish the required scanner contract.

Before production use, port descriptor-relative no-follow traversal and
identity rechecks. Retain scan-time identities for directories and all
actionable leaves. Retain cloud entries with their observed local blocks.
Add subtree coverage for errors, skipped volumes, depth and resource limits.
Publish completed real subtrees and keep cancellation distinct from success.
Bound retained tree resources and hardlink tracking. Preserve Data-volume
aliases and the app's accepted-command, generation, and progress rules.
Keep the guarded Trash executor as the only execution path.

No replacement benchmark is reported. The brief requires that benchmark
only if the production scanner switches. The upstream benchmark does not
establish accounting or safety parity for this import.
