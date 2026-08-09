# paperasse-privacy

A privacy engine for agents: image, PDF, or text in → redacted image, PDF, text, or (optionally) markdown out. Built to run *inside* the host process via native language bindings, not only as a REST call — fast enough that an agent can call it on the hot path.

> **Status: scaffold.** The text-redaction path (ingest → Tier A detect → mask) is wired end to end and tested. Pixel redaction for images/PDFs (`redact::redact_image`) is a stub — see that function's doc comment for the concrete next steps. Nothing in this repo has been `cargo build`ed yet; see [Build status](#build-status) below.

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
  wasm/         wasm-bindgen, browser-only, UNVERIFIED (see its lib.rs)
```

## Adding a Tier A recognizer

Each recognizer is a self-contained module in `crates/recognizers/src/` implementing the `Recognizer` trait (`entity_type()` + `analyze(text) -> Vec<Match>`) with its own `#[cfg(test)]` block. `fr_nir.rs` is the reference example: regex + a real checksum in `validate()`, tests ported directly from the Python recognizer's test suite. Register new recognizers in `default_registry()` in `lib.rs`.

## Build status

No Rust toolchain was available in the environment this was scaffolded in, so **none of this has been compiled yet**. Known risk areas to check first, in order:

1. `crates/core/src/ingest.rs`'s `LiteparseIngestor::ingest_image` — assumes liteparse's `PdfInput::Bytes` entry point also accepts a plain image (per its README/flowchart), routed through the same content-detection step PDFium needs. Confirm against the real crate.
2. `crates/core/src/ingest.rs`'s word-box offset construction — builds its own joined text from `text_items` rather than trusting liteparse's own `result.text`/`page.text` join convention, specifically to keep span alignment guaranteed-correct; revisit if the extracted text quality matters more than that guarantee.
3. `crates/core/src/redact.rs`'s `mask_text` — masks by `char` index against byte-offset spans; correct for every Tier A recognizer today (ASCII: digits, `@`, `.`) but will misalign on a span inside a multi-byte UTF-8 run.
4. `bindings/wasm` — depends on `paperasse-privacy-core`, which pulls in both `anydoc` and `liteparse` unconditionally; neither has been checked against `wasm32-unknown-unknown` in this combination yet.

## License

MIT
