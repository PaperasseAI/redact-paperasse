# redact-paperasse

A privacy engine for agents: image, PDF, office document, or text in → redacted image, PDF, or text out. For the text path, markdown is the default output shape (agents parse it best) — pass `markdown: false` to get plain text back instead. Built to run *inside* the host process via native language bindings, not only as a REST call — fast enough that an agent can call it on the hot path.

**Try it: [redact.paperasse.ai](https://redact.paperasse.ai)** — upload text, an image, or a PDF, pick which kinds of PII to redact, and get the redacted file back. Nothing is stored: the upload is redacted in memory and streamed back in the same request. Names and addresses (NER-backed, beta) are available alongside the checksum-anchored identifiers. Source for the demo is at [PaperasseAI/redact-paperasse-demo](https://github.com/PaperasseAI/redact-paperasse-demo).

![redact-paperasse: image, PDF, and text/markdown in, redacted out](assets/demo.gif)

*Every frame is real tool output — the ID card's SSN and the PDF are redacted by the actual CLI, not mocked up. All identifiers shown are synthetic.*

> **Status: working, shipping.** Published on [crates.io](https://crates.io/crates/redact-paperasse-cli), [npm](https://www.npmjs.com/package/redact-paperasse), and [PyPI](https://pypi.org/project/redact-paperasse/). `cargo test --workspace --locked` passes (105 tests) and clippy is warning-free on the native host and `wasm32-unknown-unknown`. The full ingest → detect → redact pipeline runs for text, images, PDFs, and office documents, including pixel redaction — validated repeatedly against real photographed French documents, and hardened by what those documents kept finding: see [docs/BUGLOG.md](docs/BUGLOG.md) for every real defect and what it taught.

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
               Tier B: opt-in REST call to a Presidio deployment for NER
               (names, locations) — its spans are routed onto the same
               OCR word boxes Tier A uses, so NER results get real
               pixel boxes on images and PDFs too
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

Neither library is forked or modified — both are consumed as ordinary crates.io dependencies through their public APIs.

### Why two detection tiers, not one

Regex+checksum recognizers (SSNs, the French NIR, IBANs, emails — a fixed identifier format with a real validation algorithm) are cheap, deterministic, and portable to any language with zero ML dependency. General NER (names, locations, anything context-dependent) needs a real NLP stack and is meaningfully less certain — in testing against a real document while building the `FR_NIR` recognizer for Presidio (on the [`paperasse-fr-nir` branch](https://github.com/PaperasseAI/presidio/tree/paperasse-fr-nir) of `data-privacy-stack/presidio`), the checksum-validated match was the reliable signal; the NER layer's guesses were the wrong ones.

So Tier A — the regex+checksum layer — is the default, in-process, zero-network path (`crates/recognizers`). Tier B is an explicit opt-in REST call to a Presidio deployment (`--tier-b` on the CLI, `tierB: { analyzerUrl, language }` in the Node binding), for when Tier A's coverage genuinely isn't enough. Both tiers accept the same two filters (`entities`, `score_threshold`), matching Presidio's own request fields.

Three Tier B policies are deliberate and worth knowing before turning it on:

- **Fail-closed on the network.** If the analyzer is unreachable, the call errors rather than silently degrading to Tier-A-only output that *looks* fully redacted but isn't.
- **Fail-closed on placement.** Tier B spans are routed through the same OCR word-box union Tier A uses, which is what lets a PERSON found in OCR text get a real black box on an image or PDF. If a span can't be matched to any word box on a pixel output, the whole run errors, naming the entity types that would have been silently visible.
- **Tier A wins overlaps.** Presidio also flags emails and NIR-shaped numbers; a Tier B span overlapping any Tier A span is dropped, so the checksum-validated match is the one reported.

Presidio's offsets count characters (it's Python); this crate counts bytes. The conversion happens once at the API boundary, and spans are clamped and boundary-snapped before masking, so a buggy or hostile analyzer can neither shift boxes on accented text nor crash the redactor.

## Repo layout

```
crates/
  core/         redact-paperasse-core — the pipeline (ingest/detect/redact)
  recognizers/  redact-paperasse-recognizers — Tier A: FR_NIR, EMAIL_ADDRESS,
                US_SSN, IBAN_CODE, CREDIT_CARD, PHONE_NUMBER,
                EU_VAT, FR_SIREN, FR_SIRET
  cli/          `redactpapr` binary (--features tier-b for the Presidio flag)
bindings/
  node/         napi-rs (redact-paperasse on npm) — see example.mjs
  python/       PyO3 + maturin (redact-paperasse on PyPI)
  wasm/         wasm-bindgen, browser-only — Tier A text redaction only
                (liteparse has no PDFium-to-raster path on wasm32, so pixel
                redaction can't exist there; text redaction is what genuinely
                benefits from running client-side anyway)
```

## Adding a Tier A recognizer

Each recognizer is a self-contained module in `crates/recognizers/src/` implementing the `Recognizer` trait (`entity_type()` + `analyze(text) -> Vec<Match>`) with its own `#[cfg(test)]` block. Register new recognizers in `default_registry()` in `lib.rs`.

Nine are registered today, each honest about how strong its own validation actually is:

- **`fr_nir.rs`** / **`iban.rs`** / **`credit_card.rs`** — a real checksum (`FR_NIR`: INSEE mod-97, `IBAN_CODE`: ISO 7064 mod-97, `CREDIT_CARD`: Luhn *plus* an allocated issuer prefix at a length that network issues — Luhn alone matched every French SIRET). A match that fails the checksum is rejected outright rather than reported at low confidence, so anything returned scores `1.0`.
- **`eu_vat.rs`** — EU VAT numbers for all 27 member states. Reports two confidence levels on purpose: `1.0` where the check digits were actually verified (FR, BE, NL, LU) and `0.8` where only the country code and body shape were checked — reporting `1.0` for the rest would assert a check that never ran. A *failed* checksum is rejected outright rather than downgraded. Note that `FR40303265045` contains the SIREN `303265045`, so redacting a company number without its VAT number republishes it on the next line.
- **`fr_siren.rs`** — SIREN and SIRET, Luhn-validated plus an anchor beyond the checksum: a nearby label (`SIRET`, `SIREN`, `RCS`, `EUID`) for both, or — for SIRET only — the canonical 3-3-3-5 display grouping, which French typesets nothing else as. SIREN deliberately does *not* accept grouping alone: `123 456 789` is exactly how French writes millions, and one in ten such amounts passes Luhn. The demo leaves both unticked by default: a SIREN is public data and legally required on invoices, so redacting it should be chosen, not assumed.
- **`us_ssn.rs`** — no checksum exists for SSNs; `validate()` instead encodes the same area/group/serial exclusion rules Presidio's own `UsSsnRecognizer` uses. Scores `0.85` to reflect structural plausibility, not a real checksum.
- **`phone_number.rs`** — French national five-pair format (each separator individually optional, because OCR merges them unpredictably), US groupings, and `+`-international. Every match must carry at least one non-digit anchor; fully compact runs are refused on purpose — a bare digit run is too ambiguous with account numbers, and that trade is documented at the pattern. Scores `0.75`.
- **`email.rs`** — regex only, no validation beyond the pattern itself. Scores `0.9`.

## Behaviours worth knowing

Each of these exists because a real document failed without it — the full stories, with measurements, are in [docs/BUGLOG.md](docs/BUGLOG.md):

- **Orientation and skew recovery.** EXIF orientation is applied once at the pipeline entry. If an image then yields *no* detections, the engine retries at 90/180/270 and a coarse skew sweep (±10/15/20°), keeping whichever orientation actually reads — a page photographed at a dozen degrees on a desk otherwise OCRs as blank while looking upright to the person who shot it. Only the empty result pays for this; the output also comes back upright.
- **Tiled OCR second pass.** Tesseract's page segmentation can skip a small dense block on a busy page entirely (not a resolution issue — measured). When pixel boxes are needed, images get a supplementary overlapping-tile OCR pass, merged by box so duplicates don't inflate the report.
- **Redacted PDFs are rasterized on purpose.** Burning pages to pixels destroys the text layer, metadata, annotations, form-field values and hidden content in one indiscriminate pass — for a privacy tool that beats surgically editing a content stream and hoping every side channel was found. Pages render at a uniform, honest DPI (`--dpi` scales them; printpdf's save-time byte budget, which used to silently downscale pages, is disabled) and recompress as q0.90 JPEG so a two-page document is ~0.3MB, not 13MB. Agents who need clean text use the markdown path, which never rasterizes anything.
- **JPEG in, JPEG out.** Photographed paperwork is overwhelmingly JPEG, which has no alpha channel; redaction drops alpha when the target format can't encode it instead of failing.
- **The score is a claim.** `1.0` means a checksum ran and passed; anything less means structural plausibility. A failed checksum rejects the match outright — failing a check is stronger evidence than not running one.

## Build status

Clean, zero warnings, on everything CI checks (`.github/workflows/ci.yml`) on every push:

- `cargo metadata --locked` first — any unlocked cargo command silently rewrites the lockfile, so the freshness check must precede all of them.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-features --all-targets --locked -D warnings`, `cargo test --workspace --locked` — native host.
- `cargo clippy -p redact-paperasse-wasm --target wasm32-unknown-unknown -D warnings` — the browser binding, checked against the actual wasm32 target.
- `cargo check -p redact-paperasse-cli --features tier-b` — the Presidio-calling path, off by default.
- The Node binding is built for real (`napi build --platform`) and exercised with `example.mjs` against the actual compiled addon — this fixture-based check is what caught the multi-token bounding-box bug (see the buglog).

## Publishing

All three registries are live and driven entirely by a `vX.Y.Z` tag push (`.github/workflows/publish.yml`):

- **crates.io**: [`redact-paperasse-recognizers`](https://crates.io/crates/redact-paperasse-recognizers), [`redact-paperasse-core`](https://crates.io/crates/redact-paperasse-core), [`redact-paperasse-cli`](https://crates.io/crates/redact-paperasse-cli) — `cargo install redact-paperasse-cli` gets you `redactpapr`.
- **npm**: [`redact-paperasse`](https://www.npmjs.com/package/redact-paperasse) — a JS/TS root package plus per-platform packages via `optionalDependencies`, each bundling the PDFium runtime library next to the `.node` addon (it's `dlopen`ed at runtime, so no packager bundles it automatically).
- **PyPI**: [`redact-paperasse`](https://pypi.org/project/redact-paperasse/) — abi3 wheels with PDFium bundled into the wheel the same way.

To release: `scripts/bump-version.sh X.Y.Z`, commit, tag, push the tag. Do not bump versions by hand — the script exists because hand-bumping broke two releases identically (manifests moved, lockfile didn't, `cargo publish --locked` refused). The npm publish loop is idempotent, so re-running a partially failed run is safe and its status means something; still check every platform package after a flaky release, because the registries are independent and have disagreed before. The first-release war stories (the PDFium wheel bug, the platform-package gap that broke Apple Silicon installs) are in the [buglog](docs/BUGLOG.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version: never commit a real
identifier, prove a bug with a failing test before fixing it, and be honest in a
recognizer's score about how strong its validation actually is.

## License

MIT — see `NOTICE.md` for a full audit of the dependency tree's own licenses (all permissive; no GPL/AGPL/SSPL anywhere in it, checked by CI's `license-check` job on every push).
