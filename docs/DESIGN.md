# scanwright — design notes

Living document. Rulings carry their date and their reason; rejected options
keep their corpse so they can be re-litigated on evidence.

## Origin (2026-09-17)

Born on a Waveshare RP2350-Touch-LCD-4.3B (RP2350B, 800x480 ST7262 RGB panel
with **no frame memory**, 8 MB QSPI PSRAM) running an oven controller UI in
Slint. The conventional design — RGB565 framebuffer in PSRAM, streamed out
through the RP2350 XIP streamer — worked but lived at the edge:

* scan-out permanently consumed ~37 MB/s of a measured 54 MB/s QMI ceiling;
* the streamer has the lowest QMI priority, so every core-0 flash-fetch storm
  (network traffic) stalled it 0.4–1.6 ms; a 32-line SRAM ring and an 88 MHz
  flash overclock were needed just to survive a ping flood (29 resyncs / 2000
  frames);
* the 90°-rotated UI turned every repaint into column writes to PSRAM.

Ruling (Ben): stop fighting bandwidth. The most compressed form of a UI is its
abstract representation; expand it to pixels on demand, on a dedicated core.

### Why not Slint's line renderer (rejected)

Slint's software renderer *is* internally a per-line display-list expander, but
the scene is private and rebuilt on every `render_by_line` call, the whole
toolkit is single-threaded/`!Send` (UI logic would have to live on the
hard-real-time core), it overdraws, and it runs from flash. Not measured "too
slow" — structurally the wrong split.

### Other rejected shapes

* RGB565 framebuffer in PSRAM + line rendering in SRAM: still 70 % bus share.
* 8-bpp indexed framebuffer in PSRAM: 35 % bus share, still PSRAM on the RT path.
* 4-bpp indexed framebuffer in SRAM (192 KB): needs a 16-colour UI; ours has ~30.
* 8-bpp in SRAM: 384 KB does not fit a 520 KB part.

## Spike result (2026-09-17, on hardware)

RP2350 @ 264 MHz, 22 MHz pclk, line budget 37.3 µs, frame 18.6 ms (54 fps),
rasterizer + lists + glyph atlas all in SRAM, 32-line ring between the
rasterizer and the PIO/DMA scan-out.

| scene | items | worst line | core-1 busy / frame | list build (core 0) | underruns |
|---|---|---|---|---|---|
| oven home page | 201 | 16 µs | 4.5 ms (24 %) | 1.2 ms | 0 |
| full-screen 21 px text, LUT masks | 1164 | 33 µs | 9.2 ms (49 %) | 2.2 ms | 0 |
| full-screen 21 px text, true blend | 1164 | 48 µs | 13.9 ms (75 %) | 2.2 ms | 0 |

Identical under a 200 Hz x 1200 B ping flood plus an HTTP fetch loop. Frame
busy time varies < 0.2 %. The blend stress case exceeds the line budget on its
worst lines and the ring absorbs it (min lead 26 of 30 lines).

Derived unit costs (rough): fill ≈ 1 cycle/px, LUT mask ≈ 12 cycles/px, blend
mask ≈ 20 cycles/px. The mask figure is higher than it needs to be — see
"Known cheap wins".

## Architecture

### Core (exists)

* `DisplayList`: fixed capacity, allocation-free, **already in panel space** —
  rotation and clipping are the builder's job, never the rasterizer's.
* Two primitives: solid fill; 4-bit coverage mask (glyphs, rounded-rect
  corners). A mask over a *known flat background* resolves through a 16-entry
  colour LUT built on the soft side — no blend math, no read-back on the hard
  side. Unknown background falls back to a true RGB565 blend.
* `Raster::line(list, y, out)`: painter's algorithm over a z-sorted active
  list maintained incrementally. RAM-resident on bare metal, calls nothing.
  Lines can be *skipped* cheaply: a missed deadline costs stale lines for one
  frame, not the frame.
* Fonts baked at build time, pre-rotated into panel space, 4-bit coverage.
* Handoff: lists are double-buffered and adopted at vsync.

### Rulings

* **Name: scanwright.** (2026-09-17)
* **Sibling to damascene, no shared code.** Share the agent-friendly authoring
  vocabulary within limits; the constraints are too different to share
  implementation. (2026-09-17)
* **Line-sink abstraction now, GRAM panels later.** The rasterizer's output
  goes through a line-sink trait so panels *with* frame memory (SPI/ST7796
  class) can be driven by diffing lists and sending only dirty lines.
  Designing the seam is in scope; implementing that backend is not yet.
  (2026-09-17)
* **`no_std`, no heap on the target; memory statically defined.** Put limits
  on what can happen at runtime so capacities are derived or declared, not
  over-allocated. (2026-09-17) — *mechanism still open, see below.*
* **License: MIT OR Apache-2.0**, matching damascene. (2026-09-17)
* **Cost model as a first-class verifier.** Per-line cost is computable from
  the list (fill px, mask px, items crossing). The simulator/CI fails a screen
  that exceeds the panel's line budget before it reaches hardware — the
  analogue of damascene's lint system. (2026-09-17)

### Planned crates

* `scanwright-core` — list, rasterizer, font runtime types, cost model. `no_std`.
* `scanwright-bake` — build-time font/icon baking (currently a `build.rs`
  inside core; to be split out so the *application* declares fonts, sizes,
  charsets, rotation and scale).
* `scanwright-ui` — El vocabulary, layout, widgets, theme, events, hit list.
* `scanwright-sim` — desktop window running the same rasterizer: pixel-exact,
  golden-image tests, cost-model reports.
* `scanwright-rp2350` — PIO + DMA RGB scan-out and the core-1 pump, safe
  writer/reader list handoff. (Today this lives in the Raven firmware repo.)

### Planned primitives

* Ring masks (borders). Icons = SVG baked to coverage masks (no new primitive).
* **Span tables** — one `(x0, x1)` per panel line: charts, circles, arbitrary
  shapes at nearly zero cost.
* Translucent scrims resolved *in the builder* (recolour what is underneath at
  emit time) — zero real-time cost.
* RGB565 images later; flash reads on the real-time path need measuring first.

### Known cheap wins (not taken yet)

* Text-run items instead of one 20-byte item per glyph (~3x smaller lists,
  far less active-list churn).
* Byte -> two-pixel LUT, or run-length glyph encoding (half of a glyph box is
  empty, a third is solid).
* Line repeat when the active set is unchanged and all fills (common in
  landscape UIs, rare in rotated ones).
* The scan-out ring can likely shrink from 32 lines to ~8.

## Open

* **How memory becomes static** (tree representation): see discussion in the
  project log — type-level views with compile-time capacity vs. a bump arena
  with declared bounds verified by tooling.
