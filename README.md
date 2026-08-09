# paperasse-privacy

A privacy engine for agents: image, PDF, or text in → redacted image, PDF, text, or (optionally) markdown out. Built to run *inside* the host process via native language bindings, not only as a REST call — fast enough that an agent can call it on the hot path.

> **Status: early, but building.** `cargo test --workspace` passes (16 tests: FR_NIR/EMAIL_ADDRESS recognizers, text masking including a real French-accented regression case, and the end-to-end text pipeline). The full ingest → detect → redact pipeline is implemented for text, images, and PDFs — including pixel redaction and PDF reassembly, not just the text path. See [Build status](#build-status) for exactly what's compiler-verified vs. still needs a real document/runtime check.

## Architecture

```text
Input (image | pdf | text)
   │
   ▼
[1] ingest   — anydoc (office formats + text-based PDFs, pure Rust, no OCR)
               or liteparse (scanned/image PDFs, plain images — PDFium +
               optional OCR, the only path that gives bounding boxes)
   │
   ▼
[2] detect   — Tier A: in-process regex+checksum recognizers (default,
               zero network hop — see crates/recognizers)
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

So Tier A — the regex+checksum layer — is the default, in-process, zero-network path (`crates/recognizers`), meant to run inside the same process as the caller via the native bindings. Tier B is an explicit opt-in REST call to a Presidio deployment, for when Tier A's coverage genuinely isn't enough and the latency/network hop is worth it.

## Repo layout

```
crates/
  core/         paperasse-privacy-core — the pipeline (ingest/detect/redact)
  recognizers/  paperasse-privacy-recognizers — Tier A: FR_NIR, EMAIL_ADDRESS, ...
  cli/          `ppr` binary
bindings/
  node/         napi-rs (@paperasse/privacy on npm)
  python/       PyO3 + maturin (paperasse-privacy on PyPI)
  wasm/         wasm-bindgen, browser-only — Tier A text redaction only,
                see "Build status" for why pixel redaction can't exist here
```

## Adding a Tier A recognizer

Each recognizer is a self-contained module in `crates/recognizers/src/` implementing the `Recognizer` trait (`entity_type()` + `analyze(text) -> Vec<Match>`) with its own `#[cfg(test)]` block. `fr_nir.rs` is the reference example: regex + a real checksum in `validate()`, tests ported directly from the Python recognizer's test suite. Register new recognizers in `default_registry()` in `lib.rs`.

## Build status

Clean, zero warnings, on both targets that matter:

- `cargo check --workspace` and `cargo test --workspace` — native host (macOS arm64): core, recognizers, CLI, and all three binding crates. **16/16 tests pass.**
- `cargo check --target wasm32-unknown-unknown -p paperasse-privacy-wasm` — the browser binding, checked against the actual wasm32 target, not just the host target `cargo check --workspace` alone would validate.

This is real, not aspirational — getting here caught and fixed several genuine bugs, not just typos:

1. A byte-offset-vs-char-index bug in `mask_text` (was collecting into a `Vec<char>` while spans are byte offsets from `regex`) — caught by the FR_NIR recognizer's own "found within surrounding French text" test, where three accented characters shifted every mask out of position. Fixed to slice `&str` directly; regression tests added in `redact.rs`.
2. An ingestion-routing gap: a PDF with a real text layer went through anydoc (fast, but produces no bounding boxes and can't see PII inside embedded images), while `OutputFormat::Native` output always tried to draw boxes — meaning PII correctly *detected* via the text path was silently never *redacted* in the pixel output. Fixed: `Ingestor::ingest` takes a `needs_boxes` hint now; any `OutputFormat::Native` PDF always routes through liteparse regardless of whether it has a text layer, and `AnydocIngestor` actively refuses (`EngineError::Unsupported`) rather than silently under-redact.
3. printpdf 0.9.1's real `PdfDocument::save` signature (needs a `&mut Vec<PdfWarnMsg>` the docs example omits).
4. liteparse 2.11.1 itself doesn't compile cleanly for `wasm32` with default features (an unconditional `use` of a module that's correctly `wasm32`-gated at declaration, but not at the import site) — worked around via a workspace-inheritance-correct feature split (native builds opt into `tesseract` explicitly; wasm32 gets liteparse's bare default).
5. **A real upstream constraint, not a bug**: `LiteParse::screenshot_input` — which `redact_pdf_bytes` depends on for pixel-level PDF redaction — doesn't exist in liteparse's wasm32 build at all (no PDFium-to-raster path there). So pixel-level PDF/image redaction is `#[cfg(not(target_arch = "wasm32"))]`-gated; the wasm32 binding only exposes Tier A text redaction, which is what actually runs client-side in a browser anyway.

What a type check *can't* validate — still needs a real document, not just a compiler:

1. `LiteparseIngestor::ingest_image` — assumes liteparse's `PdfInput::Bytes` entry point accepts a plain image per its README/flowchart. Compiles; not run against a real image yet.
2. The DPI assumption in `redact_image_bytes` (hardcoded to liteparse's default 150, not derived from the actual OCR call) — likely wrong for a real photo, would misplace redaction boxes.
3. The `XObjectTransform`/page-sizing math in `redact_pdf_bytes` — compiles against printpdf 0.9.1's real API, but whether each page image lands scaled/positioned correctly on its `PdfPage` hasn't been checked against an actual rendered PDF.

## License

MIT
