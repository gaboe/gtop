# Product

<!-- impeccable:product-schema 1 -->

## Platform

macos

Native macOS desktop app drawn with GPUI (Metal). Not a web view. Linux and Windows builds are out of scope for now.

## Users

Developers on Apple Silicon Macs. The primary user keeps gtop open all day and looks at it when a build or test runs, or when the Mac gets loud and hot, to find which process causes it. A second use is a quick glance at CPU, memory and GPU before closing it. The fork is public, so other Mac developers are an intended audience too.

## Product Purpose

Show what the Mac is doing right now, and which process is responsible, at a cost the monitor itself does not add to. Success: the user finds the process behind fan noise, heat or a slow build within seconds, and can stop it from the same window.

## Positioning

Speed is the product. gtop aims to be the fastest process monitor there is: the lowest CPU and memory cost per refresh, the shortest time from launch to the first full table, and no dropped frames while scrolling and sorting hundreds of processes. GPUI draws the window on the GPU (Metal), and the data layer refreshes only what changed. Confirmed by the user on 2026-10-23 as the most important product feature.

"Fastest in the world" is a goal, not yet a claim. Publish it only next to a reproducible benchmark against named monitors (Activity Monitor, btop, htop, NeoHtop). Until then, show the monitor's own measured cost instead of the slogan.

Second: per-process GPU time, GPU memory and temperatures read from IOKit without sudo, and listening ports mapped to processes. Activity Monitor has no per-process GPU percentage or temperature panel; terminal tops have no native window.

## Operating Context

- Runs beside an editor, terminal and browser during builds, test runs and local servers.
- Often on a second monitor or a corner of the screen, left open for hours.
- The user sorts by CPU, memory or GPU, searches by name, PID or port, pauses the list to inspect, opens process details, and kills processes.
- The refresh interval is user-selectable.

## Capabilities and Constraints

- Stats: CPU per core, memory, swap, disk for the shown volume, network, uptime, load.
- GPU: device utilization and memory in use (Apple IOAccelerator), per-process GPU percentage.
- Temperatures grouped as CPU, GPU, SSD and battery, refreshed every 5 s.
- Process table: name, PID, user, CPU, memory, GPU, disk I/O, ports, command, start time. Environment is loaded only on demand.
- Search by name, PID, command or port. Sort by any column. Pause and resume. Kill with confirmation; a kill checks PID and start time.
- History view: which processes used the most CPU, GPU and memory over the last 1, 8 or 24 hours. Samples are recorded only while gtop runs; no background agent. Gaps when gtop was closed are shown as gaps, never filled. Requested 2026-10-23, not built yet.
- Many rows at once. The table is virtualized.
- Everything must be operable from the keyboard.
- Window management, text selection and shortcuts behave like a normal macOS app.
- The monitor's own CPU cost is a product feature. Visual effects must not add a measurable per-tick cost.
- Undecided: whether multiple color themes survive. The user did not ask to keep them.

## Brand Commitments

- Name: gtop. Fork of NeoHtop by Abdenasser, MIT license. Credit the upstream project in About.
- The user asked for a modern brutalist, cypherpunk or cyberpunk direction for the redesign (2026-10-23).

## Evidence on Hand

- Real measurements from this fork: per-tick refresh cost dropped from 68–74 ms to 7–16 ms, payload from 3.37 MB to 0.34 MB. These are developer-machine numbers, not published benchmarks.
- No screenshots, testimonials, users or download counts exist yet. Do not invent them.

## Product Principles

1. Fast first. Every feature and every visual effect pays for its cost in CPU, memory and frame time, or it does not ship.
2. Prove speed, never claim it. The app shows its own cost per refresh; public speed claims need a reproducible benchmark.
3. The culprit first. The process responsible for load is the most visible thing on screen.
4. Density over decoration. More rows and numbers per screen beat whitespace.
5. Keyboard and macOS conventions are not negotiable. Numbers are exact and labeled with units.

## Accessibility & Inclusion

Color must never be the only signal for load or temperature state. Text stays readable at the dense default size.
