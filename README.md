# paperasse-privacy

A privacy engine for agents: image, PDF, office document, or text in → redacted image, PDF, text, or (optionally) markdown out. Built to run *inside* the host process via native language bindings, not only as a REST call — fast enough that an agent can call it on the hot path.

> **Status: early, but working.** `cargo test --workspace` passes (25 tests) and `cargo clippy --all-features -D warnings` is clean on both the native host target and `wasm32-unknown-unknown`. The full ingest → detect → redact pipeline is implemented for text, images, PDFs, and office documents (DOCX/XLSX/PPTX/RTF/EPUB/ODT/CSV) — including pixel redaction and PDF reassembly, not just the text path — and both the image and PDF redaction paths have been run end to end against a real photographed French document with a correct, pixel-precise result. CI runs the full check suite on every push. See [Build status](#build-status) for the details.

## Architecture

```text
Input (image | pdf | office document | text)
   │
   ▼
[1] ingest   — anydoc (office formats + text-based PDFs, pure Rust, no OCR)
               or liteparse (scanned/image PDFs, plain images — PDFium +
               optional OCR, the only path that gives bounding boxes)
   │
   ▼
[2] detect   — Tier A: in-process regex+checksum recognizers (default,
               zero network hop — see crates/recognizers), filterable by
               entities/score_threshold (mirrors Presidio's own fields)
               Tier B: optional REST call to a Presidio deployment, for
               NER (names, locations, context-dependent entities) Tier A
               structurally can't cover
   │
   ▼
[3] redact   — mask text/markdown spans, or fill pixel bounding boxes
   │
   ▼
Output (redacted image | redacted pdf | redacted text | markdown)
```

### Why two ingestion libraries, not one

[anydoc](https://github.com/firecrawl/anydoc) and [liteparse](https://github.com/run-llama/liteparse) both touch PDFs, so naively depending on both for everything would mean two overlapping PDF parsers and liteparse's PDFium/Tesseract weight paid for documents that don't need it. Instead they're scoped to non-overlapping jobs:

- **anydoc** owns every office format (DOCX/XLSX/PPTX/RTF/EPUB/ODT/CSV) plus text-based PDFs. Pure Rust, no native binary dependency, ~5ms median. This covers the large majority of real documents.
- **liteparse** owns *only* what anydoc structurally can't do: scanned/image-only PDFs and plain images, via PDFium + optional OCR (bundled Tesseract, or a pluggable HTTP OCR server). This is also the only ingestion path that produces bounding boxes, which is what makes pixel-level redaction placement possible at all.

Neither library is forked or modified — both are consumed as ordinary crates.io dependencies through their public APIs (anydoc's `to_markdown_bytes`; liteparse's `LiteParse::parse_input` → `ParseResult { pages, text, .. }` with each page's `text_items` carrying `(x, y, width, height)`).

### Why two detection tiers, not one

Regex+checksum recognizers (SSNs, the French NIR, IBANs, emails — a fixed identifier format with a real validation algorithm) are cheap, deterministic, and portable to any language with zero ML dependency. General NER (names, locations, anything context-dependent) needs a real NLP stack and is meaningfully less certain — in testing against a real document while building the `FR_NIR` recognizer for Presidio (on the `paperasse-fr-nir` branch of `data-privacy-stack/presidio`), the checksum-validated match was the reliable signal; the NER layer's guesses (misclassifying an account number as a date, a dossier number as a UK health-service number) were the wrong ones.

So Tier A — the regex+checksum layer — is the default, in-process, zero-network path (`crates/recognizers`), meant to run inside the same process as the caller via the native bindings. Tier B is an explicit opt-in REST call to a Presidio deployment, for when Tier A's coverage genuinely isn't enough and the latency/network hop is worth it. Both tiers accept the same two filters (`entities`, `score_threshold`), matching Presidio's own `analyzer_entities`/`score_threshold` request fields — see `Engine::process`.

Tier B is fail-closed by design, not fail-open: if the Presidio deployment is unreachable, the call errors rather than silently degrading to Tier-A-only output that *looks* fully redacted but isn't. It's also currently scoped to text input only — it never has pixel coordinates (`Entity::bbox` is always `None` from Tier B), so a `--tier-b` image/PDF redaction request refuses outright instead of silently skipping the entities it can't place on the page.

## Repo layout

```
crates/
  core/         paperasse-privacy-core — the pipeline (ingest/detect/redact)
  recognizers/  paperasse-privacy-recognizers — Tier A: FR_NIR, EMAIL_ADDRESS, ...
  cli/          `ppr` binary (--features tier-b for the Presidio flag)
bindings/
  node/         napi-rs (@paperasse/privacy on npm) — see example.mjs
  python/       PyO3 + maturin (paperasse-privacy on PyPI)
  wasm/         wasm-bindgen, browser-only — Tier A text redaction only,
                see "Build status" for why pixel redaction can't exist here
```

## Adding a Tier A recognizer

Each recognizer is a self-contained module in `crates/recognizers/src/` implementing the `Recognizer` trait (`entity_type()` + `analyze(text) -> Vec<Match>`) with its own `#[cfg(test)]` block. `fr_nir.rs` is the reference example: regex + a real checksum in `validate()`, tests ported directly from the Python recognizer's test suite. Register new recognizers in `default_registry()` in `lib.rs`.

## Build status

Clean, zero warnings, on everything CI checks (`.github/workflows/ci.yml`):

- `cargo fmt --all --check`, `cargo clippy --workspace --all-features --all-targets -D warnings`, `cargo test --workspace --locked` — native host. **25/25 tests pass.**
- `cargo clippy -p paperasse-privacy-wasm --target wasm32-unknown-unknown -D warnings` — the browser binding, checked against the actual wasm32 target, not just the host target the main job validates.
- `cargo check -p paperasse-privacy-cli --features tier-b` — the Presidio-calling code path, which is off by default.
- The Node binding is built for real (`napi build --platform`) and exercised with `example.mjs` against the actual compiled addon, not just type-checked.

This is real, not aspirational — getting here caught and fixed several genuine bugs, not just typos:

1. A byte-offset-vs-char-index bug in `mask_text` (was collecting into a `Vec<char>` while spans are byte offsets from `regex`) — caught by the FR_NIR recognizer's own "found within surrounding French text" test, where three accented characters shifted every mask out of position. Fixed to slice `&str` directly; regression tests added in `redact.rs`.
2. An ingestion-routing gap: a PDF with a real text layer went through anydoc (fast, but produces no bounding boxes and can't see PII inside embedded images), while `OutputFormat::Native` output always tried to draw boxes — meaning PII correctly *detected* via the text path was silently never *redacted* in the pixel output. Fixed: `Ingestor::ingest` takes a `needs_boxes` hint now; any `OutputFormat::Native` PDF always routes through liteparse regardless of whether it has a text layer, and `AnydocIngestor` actively refuses (`EngineError::Unsupported`) rather than silently under-redact.
3. printpdf 0.9.1's real `PdfDocument::save` signature (needs a `&mut Vec<PdfWarnMsg>` the docs example omits).
4. liteparse 2.11.1 itself doesn't compile cleanly for `wasm32` with default features (an unconditional `use` of a module that's correctly `wasm32`-gated at declaration, but not at the import site) — worked around via a workspace-inheritance-correct feature split (native builds opt into `tesseract` explicitly; wasm32 gets liteparse's bare default). Also: `default-features` can only be overridden *downward* if the workspace-level entry is already `false` — a member crate can't turn off a workspace-`true` default, only add features on top of a workspace-`false` baseline. Took a second pass to get this ordering right.
5. **A real upstream constraint, not a bug**: `LiteParse::screenshot_input` — which `redact_pdf_bytes` depends on for pixel-level PDF redaction — doesn't exist in liteparse's wasm32 build at all (no PDFium-to-raster path there). So pixel-level PDF/image redaction is `#[cfg(not(target_arch = "wasm32"))]`-gated; the wasm32 binding only exposes Tier A text redaction, which is what actually runs client-side in a browser anyway.
6. **`Input` originally had no way to represent a DOCX/XLSX/PPTX/CSV/etc. document at all** — only `Text | Pdf | Image` — despite anydoc's office-format coverage being a core part of the architecture's rationale. Discovered while writing direct tests for the ingest routing logic: there was no way to even construct a test case for it. Added `Input::Document { bytes, format: Option<DocumentFormat> }`, wired through all three `Ingestor` impls, with `OutputFormat::Native` correctly rejected for it (anydoc only ever converts *to* markdown, never back to a native office format).
7. Two real `cargo test`/`clippy` catches during the same pass: a partial-move borrow-checker error in the CLI (`cli.r#as.unwrap_or_else(...)` moved a field out from under a later `&cli` borrow — fixed with `.clone()`), and a `#[cfg(feature = "tier-b")]`-gated function called from a call site that wasn't itself feature-gated (compiled under `--features tier-b`, failed under default features) — both caught by actually running both build configurations, not just one.

### Verified against a real document, not just a compiler

`ppr` run end to end (`ingest → Tier A → redact`, `OutputFormat::Native`) against a real photographed French URSSAF letter — the exact same document used earlier to validate the `FR_NIR` Presidio recognizer this whole project grew out of — both as a **plain image** and as that same image **embedded in a PDF** (to exercise the PDF-specific render → redact → reassemble path separately). Both times: the NIR field was correctly detected (`FR_NIR`, score 1.0, real `bbox` coordinates) and the redaction box landed pixel-precise on it — nothing else on the page (name, address, other reference numbers, signature) was touched, page dimensions and layout fully intact in the reassembled PDF.

This resolved every item the README used to list as unverified:

1. `LiteparseIngestor::ingest_image` **does** correctly accept a plain image via `PdfInput::Bytes`, per liteparse's own README/flowchart — confirmed by real OCR output (`[liteparse] ocr: 2201.4ms`), not just a passing type check.
2. The DPI assumption in `redact_image_bytes` (liteparse's default 150) **was correct** — the box landed exactly on the target text, not offset.
3. The `XObjectTransform`/page-sizing math in `redact_pdf_bytes` **was correct** — the redacted page image scaled and positioned exactly onto its `PdfPage`, no cropping, stretching, or offset in the reassembled PDF.

Along the way this also surfaced a real bug in a dependency, not in this code: `tesseract-rs`'s build script downloads its language data (`eng.traineddata`, `tur.traineddata`) but only checks the file *exists* before skipping re-download, not that it's valid — an earlier network timeout during this project's own build left 0-byte stub files that `tesseract-rs` happily "found" on every subsequent build, failing at OCR *runtime* with "Failed to initialize Tesseract" rather than at build time. Fixed locally by deleting the corrupt files and `cargo clean -p tesseract-rs` to force a real re-download; worth keeping in mind if this ever recurs (e.g. after a CI runner's cache gets a similarly-interrupted first build).

## License

MIT
