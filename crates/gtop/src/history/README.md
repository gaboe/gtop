# I/O history

Schema version 4 adds network receive/send bytes, internal SSD read/write bytes,
peak sample rates in bytes per second, and free space. Each minute stores the
observed duration for each I/O counter. Charts divide bytes by that duration.
A missing counter is a gap, not zero. Existing system rows have NULL values.
Existing process rows have zero byte totals because older collectors did not
save them. No history can reconstruct earlier I/O.

On macOS, system SSD counters come from the `Statistics` dictionary of each
`IOBlockStorageDriver` whose parent reports `Medium Type = Solid State` and
`Physical Interconnect Location = Internal`. Handles are cached at startup.
Each tick reads one dictionary per SSD. Physical counters include kernel I/O
and I/O from processes which have exited. Reading below APFS avoids counting
both a container and its volumes. External disks and disk images are excluded.
Unavailable counters stay unknown. Other platforms currently report unknown
system SSD I/O. This does not measure NAND write amplification or wear.

Network totals use the same interface counters as the live network panel.
History uses cumulative bytes rather than integrating the rounded live rates.
A reset or decrease starts a new baseline. An empty or changed interface set
reports unknown. A returning interface set starts a new baseline. First samples,
pauses, sleep, restarts, and long collection gaps do not charge unobserved I/O.

Free space is a time-weighted mean of the system-volume free-space samples.
It excludes purgeable space. A first observation has a 0.001-second weight,
consistent with the existing history gauges. Rollups retain its weighted mean.
Byte totals sum, peak rates take the maximum, and observed durations sum.
The hourly process cache and ten-minute compaction use the same byte sums.
I/O-only process rows remain regular rows rather than packed zero presence.

Process bytes reuse the existing rusage deltas and their sample durations.
No additional syscall runs per process. The native collector checks process
identity with PID and start time, subtracts cumulative counters with saturation,
and drops exited records. History records only fresh process snapshots.
It distributes each delta over its valid duration and splits it at minute
boundaries. No bytes are charged before recording starts or across a pause.
A failed rusage read primes the next valid baseline when its recovery span
exceeds CPU sampling or crosses a recording session boundary. Such bytes are
dropped, not spread into later minutes. An older first-seen process only primes
its disk baseline. A process born inside the collection interval keeps its bytes.

Process counters describe disk I/O attributed to a process, across all devices.
They cannot identify the target SSD or assign kernel/cache writeback to an
original writer. Their totals need not equal physical SSD totals. A process
which starts and exits between ticks has no process row, but its physical I/O
remains in the system SSD counters. The UI uses the SSD read/write labels
beside the existing SSD panel; interpret process attribution with this limit.

The history tape supports CPU, memory, GPU, temperature, network receive/send,
SSD read/write, and SSD free space. I/O charts scale to the largest rate in
the displayed window. Free-space charts scale to the largest displayed mean.
The readout gives total bytes and peak rate, or mean free space. Process tables
have sortable SSD byte columns. Process details also show two byte tapes with
one column per ten minutes. Network and free space have no process ranking.
