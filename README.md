# scanwright

A UI stack for microcontrollers that **races the beam**: the UI is kept as a
compact display list and expanded to pixels one scanline at a time, in step
with the panel. No framebuffer — not in SRAM, not in PSRAM — so there is no
framebuffer memory to find and no scan-out bandwidth to fight for.

```text
soft core:  app state -> build -> layout -> display list (panel space) -> publish at vsync
hard core:  for each panel line: rasterize(list, y) -> line buffer -> DMA -> panel
```

Sibling in spirit to [damascene](https://github.com/computer-whisperer/damascene)
(agent-friendly declarative authoring), sharing vocabulary but no code — the
constraints are radically different.

Status: early. `scanwright-core` (display list, scanline rasterizer, baked
fonts, cost model) is proven on hardware; `scanwright-ui` (El vocabulary, fixed
arenas, layout, touch events) renders the oven home page on the host; see [docs/DESIGN.md](docs/DESIGN.md) for the
measurements, the rulings so far, and what is still open.

```sh
cargo test --release      # renders target/scene-*.png and target/ui-oven-home.png
```

## License

Dual-licensed under MIT or Apache-2.0, at your option (matching damascene).
The bundled Roboto fonts are Apache-2.0; see
`crates/scanwright-core/fonts/LICENSE-Roboto.txt`.
