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

That gap was bookkeeping, and the next section closed it.

### Measured unit costs and the active-record rasterizer (2026-09-17)

`bench::run` rasterizes 12 synthetic scenes, each dominated by one kind of
work, and reports cycles next to exact work counts; a least-squares fit gives
unit costs with <= 2.5 % residual per scene. First result, with an active list
of item *indices*: **68 cycles per item per line, +23 per text run, +36 per
glyph** — on a wall of text the bookkeeping was 60 % of the frame, more than
the pixels (LUT 6.2, blend 15.8, fill 0.78 cycles/px). Bus contention from the
live scan-out DMA: only ~5 %.

The rasterizer now unpacks each item **once**, on activation, into a 28-byte
active record (destination, length, resolved LUT pointer / colour word, a
running mask-row pointer advanced one stride per line; text runs are a small
state machine over non-overlapping glyph refs) and compacts only on lines
where an item ends. Pixel-identical output (golden hashes). Unit costs after:

| per line | fill px | LUT px | blend px | item | run | glyph | activate | record move | retire scan |
|---|---|---|---|---|---|---|---|---|---|
| 154 | 0.78 | 6.4 | 15.5 | 40 | 24 | 32 | 68-98 | 23 | 23 |

| scene (bench, clean) | before | after |
|---|---|---|
| text wall 21 px, LUT | 2.91 M cyc | 2.07 M (-29 %) |
| text wall 21 px, blend | 4.41 M | 3.41 M (-23 %) |
| +24 thin fills | 1.24 M | 0.86 M (-31 %) |
| sparse text runs | 1.16 M | 0.69 M (-41 %) |

Live demo, Home page: 5.2 -> 4.1 ms per frame; the text wall is now faster
than the original item-per-glyph spike *and* half its memory.

**A measured hazard: no multi-word copies on the real-time path.** Moving
active records with a plain struct copy compiles to `ldm`/`stm` and was ~35 %
cheaper on average — and produced rare 25-35 µs stalls on random lines (worst
line 12-19 k cycles, different every report) with the scan-out running.
Field-wise or word-wise *volatile* copies are cycle-for-cycle deterministic
(worst line identical in every report); word-wise is also the cheapest (23
cycles/record). Mechanism not understood — multi-beat bus transfers under DMA
contention are the suspect. Determinism wins: the ring absorbs cost, not
surprises. `move_record` documents it in the code.

**The cost model prices bookkeeping too.** Activations, record moves and
retire scans are counted exactly on the host (`cost::line_work` mirrors the
rasterizer) — without them the model under-predicted burst lines (card edges,
where a dozen items start at once) by 2x. With them, on the live demo:
predicted worst line 4922 cycles / 23 % of a core, measured 4916-4920 / 22 %.
(Before this change the older model had been checked on pages it was not
calibrated on — Log: predicted 53 %, measured 50 %; Settings: 22 % / 22 %. The
new predictions for those pages, 36 % and 19 %, are not yet re-measured.)

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

* **Type scale: four roles, six faces.** (2026-09-17) A real touch UI ported
  from Slint used 17 font sizes and 4 weights; every face costs atlas RAM
  (the atlas lives in SRAM so the real-time path never touches flash) and the
  differences were not visible at arm's length. The baked scale is Caption
  18 (Regular + Bold), Body 22 (Regular + Bold), Title 27 (Bold), Display 81
  (Bold, numeric charset) — 61 KB of atlas. Weights 600/800 collapse into
  Bold. Symbols the text face lacks (`✓ ✎ ▲ ▼ ⌫ ⇧`) are lent by DejaVu Sans
  at bake time, so there is no separate icon font in the API. The floor is
  five faces (drop Caption Bold; section heads become uppercase Regular with
  tracking) before it starts to look different. Sizes are physical px on a
  480 px wide portrait panel; a scale factor stays on the open list.
* **Simplify before porting: what a beam-raced UI does not do.** (2026-09-17,
  ruled for the first application) Word wrap becomes explicit line breaks in
  the copy (the panel is fixed and the copy is authored in code); elision
  becomes clipping until a name is seen to clip; corner radii collapse to
  three plus a pill rule (radius = half the height); no alpha anywhere — a
  scrim is an emit-time recolouring of the items beneath it, translucent text
  colours become fixed colours; state-specific layouts reuse the same cards.
  Kept deliberately: an on-screen keyboard (90 nodes is cheap) and curve
  graphs (span tables, which also give a live trace later).
* **Glyphs a run cannot carry are drawn after it, and always blend.**
  (2026-09-17) A text run's glyphs must not share a panel line; a kerned-in
  or clipped neighbour becomes its own mask item. It used to be pushed
  *before* the run and inherit the run's LUT, and a LUT run paints its
  known background over everything under its box — so the neighbour's pixels
  were stomped where the run glyph had zero coverage (found by the LUT/blend
  golden hashes diverging when the 22 px body font made "fo" overlap by one
  line). Now such glyphs are deferred until after the run item and blended,
  which is exact whatever is underneath. They are rare, so the cost is noise.

### Planned crates

* `scanwright-core` — list, rasterizer, font runtime types, cost model. `no_std`.
* `scanwright-bake` — build-time font/icon baking (currently a `build.rs`
  inside core; to be split out so the *application* declares fonts, sizes,
  charsets, rotation and scale).
* `scanwright-ui` — El vocabulary, layout, widgets, theme, events, hit list.
* `scanwright-sim` (exists, 2026-09-17) — the panel in a winit/softbuffer
  window: the real rasterizer line by line, the mouse as the touch
  controller, `App::tick` as the clock, the cost model's verdict in the title
  bar on every rebuild. `Viewer::render` works without a window, so CI runs
  the same path (`tests/headless.rs`). `S` saves a PNG.
* `scanwright-rp2350` — PIO + DMA RGB scan-out and the core-1 pump, safe
  writer/reader list handoff. (Today this lives in the Raven firmware repo.)

### Primitives and widgets added 2026-09-17 (for the oven port)

* **Borders without ring masks.** `rounded_rect_bordered` = four outline
  strips + four corner masks (border colour over the known background, LUT)
  + three interior fills + four smaller corner masks that *blend*. The
  smaller masks must blend: their boxes poke past the outer arc whenever the
  border is thinner than ~0.3 r, so no single colour is under them. Nothing
  is painted twice outside the corner boxes. *Rejected — ring corner masks:*
  a new mask family in the pool for a few dozen pixels per line that blend
  anyway. *Rejected — inner rounded rect over an outer one:* overdraws the
  whole area (fills are cheap, but not free on a 448 px wide card).
* **Builder clip rect** (`set_clip`): fills, masks and run glyphs clip to a
  logical rect instead of the panel. One mechanism serves scroll viewports
  and labels wider than their box (the port's "elide becomes clip").
* **Scrims are an emit-time colour transform.** Everything painted before a
  `.scrim()` node is emitted with its colours at ~30 % (fills, text, the
  screen background); the sheet drawn after it is full colour. Zero
  real-time cost against ~7.4 k cycles per line for a blended full-width
  fill — three quarters of the WS-LCD43B's line budget. One scrim per
  screen; a second one would not dim twice (documented, not needed).
* **`scroll(..)` viewports.** A column with an unbounded main axis, clipped
  to its rect, shifted by a per-key offset that lives in the `Ui` (8 slots)
  and survives rebuilds. Touch: a press inside becomes a drag after 8 px of
  travel, cancelling the press; a drag never clicks; an overlay drawn over
  the viewport from outside it (sheet, scrim) blocks the drag. Offsets are
  clamped to the content height after layout, with one relayout when a
  shorter list left a stale offset.
* Text: `\n` breaks lines (layout measures the widest, emit issues one run
  per line); `.tracking(px)` letter spacing; `.border(w, colour)`.
* Capacities raised for the port: 32 LUTs (was 24), 3 KB mask pool and 10
  corner radii (was 2 KB / 4).

### Planned primitives

* Icons = SVG baked to coverage masks (no new primitive).
* **Span tables** — one `(x0, x1)` per panel line: charts, circles, arbitrary
  shapes at nearly zero cost. Next: the profile curve.
* RGB565 images later; flash reads on the real-time path need measuring first.

### Known cheap wins (not taken yet)

* Byte -> two-pixel LUT, or run-length glyph encoding (half of a glyph box is
  empty, a third is solid).
* Line repeat when the active set is unchanged and all fills (common in
  landscape UIs, rare in rotated ones).
* The scan-out ring can likely shrink from 32 lines to ~8.

### First real application (2026-09-17): what it measured

The oven controller's touch UI (four tabs, nine run states, six modal sheets,
an on-screen keyboard) was ported from Slint in one day on the primitives
above. Host verification walks every page and sheet, rasterizes it, prices it
and records arena peaks: 414 nodes, 1051 text bytes, 71 hits, 602 items,
431 glyphs (the keyboard over the editor), all fitting the panel with a
16-line ring. Two things the numbers said:

* **The left edge is the expensive line.** In a rotated portrait UI every
  card, row and button starts at the same logical x, so one panel line
  activates all of them: 200–350 activations at ~100 cycles each, up to
  36 k cycles (3.7 line periods) on a sheet-over-page screen whose other
  lines cost 6–8 k. The ring absorbs it, but activation is now the largest
  single cost on the worst line, not pixels. Cheaper unpacking, or lazy
  unpacking of items whose first visible pixel is further down, is the next
  rasterizer win.
* **Content under a scrim is still emitted in full.** The sheet screens carry
  the whole page beneath them (dimmed). Culling items fully covered by an
  opaque later item is the other obvious win, and would cut those screens
  roughly in half.

## Open / next

* Declared bounds on dynamic content (`max_chars`, `each(..).max(n)`) and a
  checker that prices each tree *shape* at its bounds, so capacity coverage is
  "every page and branch once", not "every state".
* Node is ~90 bytes (256 nodes = 23 KB); pack it.
* The per-key state table holds scroll offsets only; animation phase etc.
  would join it.
* Text wrapping; scale factor (layout is in physical px today); rotations
  other than 90°.
* `scanwright-bake`, `scanwright-rp2350`, the line-sink trait.
* Remaining rasterizer ideas: cheaper / lazy activation (the left-edge
  storm above), culling items under opaque later items, byte -> two-pixel
  LUT, run-length glyph rows.
* Why do `ldm`/`stm` record copies stall (see the measured hazard)?
* `Ui` and `DisplayList` statics land in `.data` (non-zero initialisers: the
  `NONE` link sentinel, the font pointers) — ~70 KB of flash image and boot
  copy for nothing. Make their `new()` all-zero.
