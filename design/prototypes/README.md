# gtop design prototypes

HTML prototypes from the 2026-10-23 redesign. Every page uses the same synthetic data (`data.js`). Nothing here talks to the real system.

**Chosen design:** `vt-timeline.html` ("VFD tower + last hour"). The native GPUI app must match it. Screenshots of every state are in `reference/`.

Open `index.html` through a local server so the shared scripts load:

```sh
cd design/prototypes && python3 -m http.server 8765
# then open http://127.0.0.1:8765/
```

| File | What it is |
| --- | --- |
| `vt-timeline.html` | Chosen. VFD window tower on the left, last-hour strip with a metric switch (CPU, memory, GPU, CPU temperature), process table, detail panel on row click, moment detail on a histogram column click. |
| `vt-tower.html`, `vt-compact.html` | Other VFD × tower variants. |
| `rack.html`, `rack-side.html`, `rack-blade.html`, `rack-vfd.html` | Earlier Rack variants. |
| `hmi.html`, `bitmap.html` | First-round directions that were not chosen. |
| `data.js` | Synthetic processes, live ticks and 24 h of minute buckets. The demo day is anchored to the current time, with a 4.5 h gap when gtop was closed. |
| `ui.js` | View, sort, search, selection, detail, settings and keyboard behavior. |
| `theme.js` | Palettes: 5 presets, a 6-color mixer, random mix and saved mixes. |
| `rack-kit.js`, `vfd-kit.js` | Shared building blocks. |
| `rack-base.css`, `vfd.css` | Shared styles and tokens. |

## Keys

`/` find, `J` `K` move, click or `Enter` detail, `Esc` close, `E` environment, `X` kill (then `Y` or `N`), `C` `M` `G` `P` sort, `Space` pause, `Tab` live or history, `1` `8` `2` history for 1 h, 8 h, 24 h, `V` last-hour metric, `T` next palette, `S` settings, `R` random palette while settings are open. Click a histogram column for the moment detail, then `←` `→` (or `H` `L`) step one column.

## Moment detail

A click on any column of the last-hour strip or the history tape opens the moment detail in the side panel. The column turns silk white.

- The header shows the span (one minute in the last hour, 5 or 10 minutes in the 8 h and 24 h tapes) and how many minutes were recorded.
- The VFD window shows average and peak CPU, memory, GPU and CPU temperature for that span. The active metric has an outline.
- The list shows the top 8 processes for that span, ranked by the active metric: core time, peak memory or GPU time. Temperature ranks by core time.
- A click on a process that still runs opens its process detail. An exited process is marked and keeps only its history.
- A click on a hatched column says gtop was not running.
- `←` `→` step one column. A second click on the same column or `Esc` closes it. In the live view, `Full hour · 1` opens the 1 h history.

References: `reference/13-moment-live.png`, `reference/14-moment-history-24h.png`.

## Units in VFD readouts

Percent and degree signs in VFD readouts are 5×7 dot-matrix characters, the way a VFD tube draws them (`KIT.unit` in `rack-kit.js`, `.u-dm` in `vfd.css`). Each is lit at digit height with unlit dots behind it, like the `888` field behind the digits. The Doto `%` glyph read as a crossed block, so it is never used. Numbers sit in a fixed-width field (for example `888` for 0 to 100), so the readout does not jump as values change. `percent-lab.html` compares the variants that were considered.

The reference images 01, 02, 07-hour-temp, 13 and 14 show the final units. The other references predate this rule; where they differ, this section wins.

## Layout changes, 2026-10-24

- **Tower order.** CPU, then memory and swap, then GPU and VRAM, then temperatures, then gtop self. The load source window is removed.
- **Per-core load.** The CPU window shows one 12-segment column per core. Performance cores and efficiency cores sit in separate banks, each with its core numbers and its average. The top three segments turn hot above 90 %. Read the P and E core counts from the system (`hw.perflevel0.logicalcpu`, `hw.perflevel1.logicalcpu`); a Mac without levels shows one bank named Cores. `cores-lab.html` keeps the variants that were considered.
- **Top ten rows.** The first ten rows of the current order are taller (30 px instead of 22 px), use 14 px text, a brighter semibold name and a larger CPU bar. A rule separates them from the rest, and the rest are dimmed.
- **Process analysis in the detail panel**, below the CPU trace:
  - *What it is*: a name, a kind (Developer tool, macOS, App helper, App, Service, Unknown) and one sentence. The source is always shown: gtop's catalog, the process path (for example "Part of Slack" from a path inside `Slack.app`), or "no match". The catalog is a data file of common macOS and developer processes.
  - *History · 24 h*: a 24 h tape for all processes with that name (10-minute columns, gaps hatched), a one-line summary of the pattern and of how now compares with its usual busy load, then core time, busy average, peak CPU with its time, longest run over 50 % and peak memory. Computed only from recorded buckets. A process that was never recorded says so.

References: `reference/01-live.png`, `reference/02-detail.png`, `reference/02b-detail-history.png`.
