---
name: gtop Rack
description: Dense native macOS monitoring with recessed VFD readouts.
colors:
  ground: "#0b0c0d"
  plate: "#141618"
  silk: "#e8e6df"
  led: "#ffb000"
  hot: "#ff3b1f"
  ok: "#38ff8b"
typography:
  display:
    fontFamily: Doto Display
    fontWeight: 900
    lineHeight: 1
  body:
    fontFamily: JetBrains Mono
    fontSize: 12px
  label:
    fontFamily: Archivo Labels
    fontWeight: 700
    fontSize: 10px
  keycap:
    fontFamily: Silkscreen
    fontSize: 8px
rounded:
  panel: 0px
spacing:
  segment: 2px
  control: 6px
  panel: 8px
  outer: 10px
  inset: 12px
components:
  rack-button:
    textColor: "{colors.silk}"
    typography: "{typography.label}"
    rounded: "{rounded.panel}"
  tower:
    backgroundColor: "{colors.plate}"
    width: 300px
  detail-panel:
    width: 380px
---

# Design system: gtop Rack

## Overview

The visual world is a VFD rack instrument. The app runs in Operate mode.
Recessed readouts sit inside flat metal plates. Dense rows keep process names and resource values visible beside the system readings.

This document describes the native GPUI app in `crates/gtop`, not the older Svelte interface or HTML prototypes.
The default palette is Amber. Palette changes preserve the same geometry and semantic color roles.

## Colors

The frontmatter records the six Amber base colors from `palette::presets`.
`led` lights normal readings and active segments. `hot` marks high load and dangerous actions.
`ok` marks positive status. `silk` carries printed text. `plate` holds panels above `ground`.

The other shipped palettes use the same roles, in this order:

| Palette | ground | plate | silk | led | hot | ok |
| --- | --- | --- | --- | --- | --- | --- |
| Ice | #07090c | #0f141a | #dfe8f0 | #43d6ff | #ff3b6b | #b4ff39 |
| Phosphor | #050805 | #0c120d | #d6f5d9 | #39ff6a | #ff4a2e | #d8ff5a |
| Neon | #0a0710 | #130e1c | #f1e6ff | #ff2bd6 | #ffe600 | #00f0ff |
| Aluminium | #e8e9e5 | #f5f5f0 | #252a2b | #923d18 | #ac292a | #236344 |

`Base::tokens` derives secondary colors in OKLab. `mix(a, b, p)` gives `a` weight `p`.
In dark palettes, `silk_dim` mixes silk and plate at 0.58. `ink_row` uses 0.78.
`rule`, `rule_hi`, `plate2`, `well`, and `row_rule` adapt to the base ground's brightness.
Dark inactive segments mix led and ground at 0.12. Ghost digits use 0.18.
Use these derived roles when a palette changes. Fixed gray replacements lose the palette relationship.

### Aluminium light mode

Aluminium uses a warm silver ground and pale enamel plates. Charcoal labels and
burnt-orange readings keep the rack legible in daylight. Red marks high load;
forest green marks positive status. Values and labels retain the same geometry.

Light readout and table wells use `well`; table headings use `table_head`.
`selection` is a quiet orange wash. `plate2` marks secondary and hover surfaces.
`silk_dim` and `cmd` retain text contrast on every light surface.
`rule_hi` supplies control outlines and the scrollbar. Quiet row rules separate
rows without competing with data. Disk selection uses `selection`, not an outline
color. Active meter ink contrasts with both the well and inactive segments.
Ghost digits remain faint guides. They do not carry values.

Light mode disables glyph, lamp, unit, and segment glow. Dark palettes retain
all existing colors and mixes. The contrast test in `palette.rs` checks text,
control outlines, status, selections, and meter ink against their real surfaces.
The ignored `aluminium_offscreen_review` test renders live, history, detail, and
disk fixtures through GPUI's headless Metal renderer. It never opens a native window.
Set `GTOP_PALETTE_RENDER_DIR` to the output directory before running that test.

## Typography

All five faces are bundled assets. Doto Display carries illuminated numbers at weight 900, with a line height equal to its size.
Readouts use sizes such as 22 px and 42 px. JetBrains Mono carries exact data, commands, and process rows.
Archivo Labels carries compact uppercase labels, navigation, and outlined controls.
Archivo Display carries the selected-process heading at weight 800 and 30 px.
Silkscreen carries keyboard hints and the tower's self-monitor values.
Core labels use JetBrains Mono at 8 px. Table rows use 12 px data text, with larger text in priority rows.

The percent and degree signs use authored 5 by 7 canvas paths, not font glyphs.
The canvas box uses a top offset of 0.155, a height of 0.69 times the readout size, and a width-to-height ratio of 10:14.
These values describe the canvas box, not the ink bounds of both signs.
The percent ink spans 13.64 canvas units. The degree sign lights dots only in its first four rows.
`UNIT_GLYPHS`, `unit_glyph_bounds`, and `unit_paths` own this geometry.
Other characters remain real font text.

## Layout

The default native content size is 1480 by 930 px. The minimum is 1200 by 720 px.
The root has 10 px padding and an 8 px gap. The system tower stays 300 px wide and scrolls vertically.
The process or history area fills the remaining width. A selected detail panel or the palette editor takes 380 px beside it.
There is no mobile breakpoint or stacked mobile layout.

The tower has 12 px horizontal padding, 14 px top padding, 12 px bottom padding, and 8 px gaps.
Recessed panels use 12 px horizontal padding and 10 px vertical padding.
Their default gap is 6 px. The CPU panel uses a 2 px gap.
Process rows are normally 22 px high. Priority rows are 30 px high.
History rows are 22 px high, beneath a 29 px header. Columns can resize and scroll horizontally.

## Elevation & Depth

Depth comes from dark readout wells, thin borders, small screw details, and lit foreground marks.
Readout wells mix ground with black at 0.8 and have a black 1 px border.
A faint dot grid sits on a 3 px pitch behind the readings.

Glow is part of the VFD world. It has zero offset and does not imply a floating card.
`glow.rs` rasterizes font outlines into cached blurred glyph images.
Display glow uses an 8 px radius. Mono glow uses 6 px.
Normal display opacity is 0.60, hot display opacity is 0.65, and mono opacity is 0.45.
Canvas unit glyphs use an 8 px glow. Active meter segments use a 4 px glow.
Glow is disabled in the light palette. The foreground glyph remains sharp above the halo.

## Shapes

Panels and controls have square corners. Borders are thin and use derived rule colors.
Lamps are 6 px squares. Meter segments are rectangles with small gaps.
The percent and degree glyphs keep their authored dot geometry at each readout size.

## Components

### Readouts and meters

`lit` layers a dim ghost value behind sharp Doto text and cached glow.
`lit_unit` adds the authored unit glyph on the same baseline.
Horizontal meters have 2 px gaps and an 8 px height. Priority process rows use 12 px meter height.
The final 15 percent of each meter uses hot color.
Core meters use twelve vertical segments in a 36 px column, with 1 px vertical gaps.
Performance and efficiency banks have separate averages and P1 or E1 numbering.
Each bank has a 40 px flex basis, a 96 px minimum, and grows in proportion to its core count. Core labels stay on one line.
When topology cannot be verified, one generic bank shows numeric labels.

### Process and history tables

Rows use the shared column-width model. Exact values align within their columns.
CPU bars use one core as full scale: 100% fills the bar. Values above 100% keep the
bar full and add up to four bright overflow ticks, so multi-core use remains visible.
Live process CPU bars use the same percentage shown in the CPU column. History CPU
bars use each process's peak CPU percentage. The live column heading is `CPU %`.
A CPU bar ignores negative and non-finite values.
The table virtualizes rows. A selected process row uses `selection`, which retains `mix(plate2, led, 0.7)` in dark palettes.
Selected history rows use `plate2`. Neither table adds a hover fill or a silk selection border.
Row separators use `row_rule`.
The table well and header use `table` and `table_head`. Dark palettes retain their distinct mixes of ground and plate.

### Buttons and inputs

Rack buttons are outlined, square, and use bold printed labels.
Input wells, focus rings, primary actions, and dangerous actions receive their colors through `theme::apply`.
The theme accent uses `plate2`. Primary actions and the focus ring use `led`.
Dangerous actions use `hot`. Secondary controls use `plate` and `plate2`.

### Navigation and side panels

Live and history controls sit above the main area. Keyboard hints use bordered keycaps.
The tower stays visible beside process details, history moments, disk information, and the palette editor.
The palette editor exposes the six base roles and saved mixes.

## Do's and Don'ts

- Do use semantic base and derived palette roles.
- Do retain exact units, fixed-width data, and separate printed labels.
- Do reuse the authored unit glyphs and cached glow.
- Do preserve square controls and the dense row geometry.
- Don't replace the native table with the old HTML prototype styling.
- Don't add glow to the light palette.
- Don't communicate a warning through color alone.
- Don't add effects that increase the monitor's per-tick work without measured evidence.

Source files: [palette.rs](crates/gtop/src/palette.rs), [theme.rs](crates/gtop/src/theme.rs),
[rack.rs](crates/gtop/src/rack.rs), [glow.rs](crates/gtop/src/glow.rs),
[process_table.rs](crates/gtop/src/process_table.rs), and [main.rs](crates/gtop/src/main.rs).
