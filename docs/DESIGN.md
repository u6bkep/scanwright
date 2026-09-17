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

### After text runs + the UI layer (same day, same hardware)

| scene | list | worst line | core-1 busy / frame | build (core 0) | underruns |
|---|---|---|---|---|---|
| oven home page, authored in `scanwright-ui` | 69 items + 152 glyphs | 19 µs | 5.3 ms (28 %) | 1.7 ms (build + layout + emit) | 0 |
| full-screen text, LUT | 32 items + 1162 glyphs | 36 µs | 11.8 ms (63 %) | 1.4 ms | 0 |
| full-screen text, blend | 32 items + 1162 glyphs | 52 µs | 16.9 ms (91 %) | 1.4 ms | 0 (min lead 23/30) |

**Text runs bought memory and cost time.** One item + a 12-byte ref per glyph
(vs a 20-byte item per glyph) roughly halves text-heavy lists and shrinks the
active set, but the text-heavy scenes got ~25 % slower on core 1 than the
per-glyph-item spike (9.2 -> 11.8 ms, 13.9 -> 16.9 ms). Three variants were
measured, none recovered it:

* 6-byte refs indexing the font's glyph table: worst (12.7 / 17.5 ms).
* self-contained 12-byte refs, all pixel loops inlined: 11.5 / 18.0 ms and the
  blend scene **underran** (register spills inside the blend loop).
* all pixel loops out of line: 12.5 / 17.2 ms — each call costs ~20 cycles per
  item per line, which hurts fills and LUT rows more than tidy codegen helps.
* **kept:** fill + LUT inline, blend out of line: 11.8 / 16.9 ms.

The remaining gap is per-run, per-line overhead (a run is active on every
line it spans, including the gaps between its glyphs, and its cursor state
spills). This wants a cycle-accurate harness (a Cortex-M33 bench binary, DWT
counts per primitive), not more flash-and-look. Ordinary UI pages are
nowhere near the limit, so it is parked, not forgotten.

`cost::CostModel::CORTEX_M33` is calibrated to this table: whole-frame load
within +/-5 % (slightly conservative), worst line within ~15 %.

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
* **`no_std`, no heap on the target; capacities declared, embassy-arena
  style.** (2026-09-17) The element tree is built into fixed arenas owned by
  `Ui<NODES, TEXT, HITS>`; display lists are `DisplayList<ITEMS, GLYPHS>`. The
  application picks the numbers; every rebuild returns a `BuildReport` of what
  it used, overflow drops elements and says so, and the host tooling is where
  the numbers get verified (next to the cost model, which needs the same
  screen coverage anyway).
  *Rejected — type-level views with compile-time-derived capacities*
  (`impl View`, tuples, `either`/`select!`): it is the only design that makes
  memory a compile-time fact, but (a) the line-time budget can only ever be
  verified by running screens, so it guarantees the less dangerous half of the
  resource problem; (b) its authoring tax (no heterogeneous arrays, `match`
  arms needing macros, trait-bound errors) is paid on every UI ever written,
  while sizing is paid once per product; (c) data-driven screens are awkward as
  types; (d) it monomorphizes layout/emit per screen on a size-optimized
  target. Embassy made the same trade: type-derived task storage on nightly,
  a declared arena on stable — and the arena is fine because its failure is
  loud. It stays available as a front end over the same core if a product
  ever needs the hard guarantee.
* **`El` is a handle; builders are free functions; the tree being built is
  ambient.** (2026-09-17, Claude's call — flagged for Ben's review.) `El` is a
  2-byte index into the tree under construction, so authoring code has no
  lifetimes and no context parameter and reads like damascene's:
  `column([h1("Oven 1"), button("+").key("inc")])`. The cost is one piece of
  ambient state (`current.rs`): a pointer that is set only inside
  `Ui::rebuild`, thread-local on `std`, a single claimed global on the target;
  building outside a rebuild, nesting rebuilds, or rebuilding from two cores
  panics. *Rejected:* explicit `cx: &BuildCx<'a>` returning `El<'a>` — every
  helper function grows a lifetime and a parameter, and modifiers on a handle
  still need the tree. If the ambient state ever bites, switching is a
  mechanical API change; nothing below the builders depends on it.
* **Known-background tracking is automatic.** Emit carries the nearest
  ancestor fill down the tree, so text and rounded corners become LUT masks
  without the author knowing the distinction exists; later children of a
  `stack` blend. (2026-09-17)
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

* Byte -> two-pixel LUT, or run-length glyph encoding (half of a glyph box is
  empty, a third is solid).
* Line repeat when the active set is unchanged and all fills (common in
  landscape UIs, rare in rotated ones).
* The scan-out ring can likely shrink from 32 lines to ~8.

## Open / next

* Declared bounds on dynamic content (`max_chars`, `each(..).max(n)`) and a
  checker that prices each tree *shape* at its bounds, so capacity coverage is
  "every page and branch once", not "every state".
* Node is ~90 bytes (256 nodes = 23 KB); pack it.
* Persistent per-key widget state table (scroll offsets, animation phase).
* Text wrapping; scale factor (layout is in physical px today); rotations
  other than 90°.
* `scanwright-sim` (window), `scanwright-bake`, `scanwright-rp2350`, the
  line-sink trait.
* Rasterizer micro-optimisation with a cycle-accurate bench (see the text-run
  measurements above); byte -> two-pixel LUT; run-length glyph rows.
* `Ui` and `DisplayList` statics land in `.data` (non-zero initialisers: the
  `NONE` link sentinel, the font pointers) — ~70 KB of flash image and boot
  copy for nothing. Make their `new()` all-zero.
