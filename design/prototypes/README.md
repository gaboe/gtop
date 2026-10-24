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
