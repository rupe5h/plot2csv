# plot2csv — plot digitizer

Extract numerical data from images of plots, in the spirit of WebPlotDigitizer,
with an automatic pipeline that you review and correct at every step. Written in
Rust with egui; ships as **one self-contained HTML file** that works offline
(open it straight from disk). Nothing is uploaded anywhere.

## Pipeline

Each stage proposes a result automatically. You review it, edit it in the UI, and
press **Approve & continue** (or Enter); only then does the next stage run.
Editing an approved stage marks later stages *stale*. You can then re-run their
automatic step or keep your edits. **Edit → Undo / Redo** (Ctrl+Z, Ctrl+Y or
Ctrl+Shift+Z; Cmd on macOS) steps back through every change, including approvals
and automatic re-runs; loading an image or project starts a new history.

| Stage | Automatic proposal | What you can edit |
|---|---|---|
| ① Import | — | load (file, drag-and-drop, Ctrl+V), optional photo enhancement |
| ② Correct | rotation and perspective, from the vanishing points of the long line families | drag the 4 corners, rotate slider, 90° turns, output size, preview |
| ③ Plot area | framed boxes and L-shaped axes, multi-panel figures, outer box with labels, bar-chart guess | add, delete, resize panels; plot type per panel |
| ④ Text | indra OCR of each panel, a rotated pass for Y titles, tick-guided OCR for missed labels, role classification | role, text, value per item (double-click a box to correct its text in place); read text in a drawn box; re-read |
| ⑤ Axes | tick/grid detection plus RANSAC fit of the labels: linear, log (incl. OCR'd superscripts `10°`, `102`), dates | WPD-style calibration points (drag, arrow-nudge) and values; all axis types |
| ⑥ Data | colour clustering with grid-line removal; markers (blob centroids); lines (multi-curve tracking that separates same-colour solid/dashed/dotted curves through crossings, and joins a spiky curve the legend shows once), all sampled on one X grid per panel; bars with category labels (the X-tick row, never the axis title); legend names (wrapped entries joined) in legend order | WPD manual tools (add/adjust/delete, Q/W, arrows) and automatic tools (mask box/pen/erase, shown by default — the pen takes the colour of the curve it paints over, colour picker, dominant colours, distance, Averaging Window, X Step (± interpolation), Blob, Bar, Multi-track); the colour test leaves out pixels that better match another plot colour (grid, other series) |
| ⑦ Export | — | Acquired Data view of one series (sort raw/X/Y/nearest-neighbour) or all series side by side (bar charts: always, one row per category), number format, separator; axis titles as column headers; CSV, long/wide CSV, ZIP, project JSON |

Plot types: 2D X-Y (linear, log, dates), bar charts, polar, ternary, map with scale
bar, and image (pixels). The **Analyze** menu measures distances and angles.
Projects save and load as JSON (File menu), with the image embedded.
**Help → Download for Offline Use** saves the page as `plot2csv.html` to open
later without a network connection.

## Building

Requirements: `rustup` (the toolchain in `rust-toolchain.toml` is installed
automatically), `wasm-bindgen-cli 0.2.130`, and `npm` (only to fetch onnxruntime-web).

```sh
cargo install wasm-bindgen-cli --version 0.2.130 --locked
cargo xtask fetch        # onnxruntime-web 1.31.1 + indra-ocr v1.1.0 models into vendor/ (SHA256-checked)
cargo xtask dist         # -> dist/plot2csv.html (~15 MB, everything inlined)
```

The HTML embeds the app wasm, onnxruntime-web (single-threaded wasm) and the indra
`det.onnx` / `rec.onnx` models as gzip+base64 payloads, decompressed at start-up.

Native desktop build (for development), with OCR through the `ort` crate:

```sh
cargo run -p plot2csv-app --features native-ocr
```

## Testing

```sh
cargo test --workspace                                   # unit tests (calibration math, CV, OCR pre/post, export)
python3 -I tools/fixtures/gen_fixtures.py tests/fixtures # synthetic charts with ground truth (needs Pillow)
cargo run -p plot2csv-ocr --profile eval --features native-ocr --example eval -- vendor/models tests/fixtures
node tools/e2e/m0_ocr.mjs   dist/plot2csv.html tests/fixtures/sine_basic.png              # OCR from file:// in headless Chrome
node tools/e2e/pipeline.mjs dist/plot2csv.html tests/fixtures/photo_sine.jpg target/e2e   # all 7 stages, screenshots
node tools/e2e/interact.mjs dist/plot2csv.html tests/fixtures/sine_basic.png target/e2e   # dialogs, manual tools, measuring
node tools/e2e/help.mjs     dist/plot2csv.html tests/fixtures/sine_basic.png target/e2e   # Help dialogs; offline copy downloads, boots and runs OCR
```

The `eval` benchmark runs the whole automatic pipeline (correction, panels, OCR,
axes, series) on every fixture and reports each series' error against ground
truth, normalised by the axis range. It also runs with `EVAL_VERBOSE=1`, which
dumps the text items, and `EVAL_DUMP=dir`, which writes overlay images. A
fixture also fails if extra series are found, or if a series' name differs from
its `legend` entry in the JSON. Current results: every fixture passes, with
median errors of 0.05–0.7 %. The fixtures are a clean chart, a photographed
chart, log axes with superscripts, markers, bars, same-colour
solid/dashed/dotted curves, a two-panel figure, and a spreadsheet-style
response spectrum (log grid whose lines darken where they cross, title and
two-line legend inside the frame, spiky curves that cross).

## Layout

```
crates/plot2csv-core    data model: geometry, calibrations (all axis types), datasets, tables, export, project/stage state
crates/plot2csv-vision  image processing: warp, correction, panels, ticks/axis inference, colour, extraction, series detection
crates/plot2csv-ocr     indra OCR pre/post-processing (port of indra_ocr), label parsing/classification, panel OCR plumbing
crates/plot2csv-app     egui app: stages, canvas, magnifier, dialogs, web/native platform layer
  web/                  HTML template and the onnxruntime-web bridge
xtask                   fetch / dist (single-HTML bundler)
tools/                  fixture generator, Chrome DevTools e2e scripts
```

## Licences

Copyright (C) 2026 the plot2csv authors.

The code is free software under the [GNU Affero General Public License v3.0 or
later](LICENSE) (`AGPL-3.0-or-later`). If you distribute plot2csv, or let people use a
modified version over a network, you must offer them its complete source code
under the same licence. The AGPL does not cover the OCR models: they are
[indra-ocr](https://huggingface.co/datalab-to/indra-ocr) by Datalab. Their weights
use a **modified OpenRAIL-M licence**: free for research, personal use, and
startups under $20M funding/revenue, and not to be used competitively with
Datalab's API. Check that this fits your use before you distribute builds.
onnxruntime-web is MIT. The bundled fonts are Inter (SIL OFL 1.1) and Hack (MIT /
Bitstream Vera), and the icons are from Font Awesome Free (CC BY 4.0); see
`crates/plot2csv-app/licenses/` and `crates/plot2csv-app/assets/`.
