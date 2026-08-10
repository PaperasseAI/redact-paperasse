//! Browser bindings. Verified against the real target:
//! `cargo check --target wasm32-unknown-unknown -p redact-paperasse-wasm`
//! passes clean (zero warnings) — see the repo README's "Build status" for
//! what that took (a Cargo workspace-inheritance fix so liteparse's
//! `tesseract` feature, which doesn't target wasm32, is off by default
//! here; `redact_pdf_bytes` is `#[cfg(not(target_arch = "wasm32"))]`-gated
//! in the core crate since liteparse's own `screenshot_input` doesn't exist
//! on wasm32 either — a real upstream constraint, not a choice made here).
//! This binding therefore only exposes Tier A text redaction, which is
//! exactly what runs client-side in a browser anyway.

use redact_paperasse_core::{Engine, Input, OutputFormat};
use wasm_bindgen::prelude::*;

/// Redact PII from plain text (Tier A: in-process regex+checksum
/// recognizers — the whole point of a WASM build, since it runs entirely
/// client-side with no server round trip). Markdown is the default output
/// (this is a tool for agents, and markdown is what they parse best) —
/// pass `markdown: false` to get back plain text in the input's own shape
/// instead. Pass `entities: ["FR_NIR"]` to redact only that entity
/// type — matches Presidio's `analyzer_entities` filter; omit/`undefined`
/// to redact every entity type Tier A's recognizers cover. Pass
/// `score_threshold: 0.95` to drop matches scoring below it — matches
/// Presidio's own `score_threshold`.
#[wasm_bindgen(js_name = redactText)]
pub async fn redact_text(
    text: String,
    markdown: Option<bool>,
    entities: Option<Vec<String>>,
    score_threshold: Option<f32>,
) -> Result<String, JsError> {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();

    let engine = Engine::default();
    let format = if markdown.unwrap_or(true) {
        OutputFormat::Markdown
    } else {
        OutputFormat::Native
    };
    let result = engine
        .process(
            Input::Text(text),
            format,
            entities.as_deref(),
            score_threshold,
        )
        .await
        .map_err(|e| JsError::new(&e.to_string()))?;
    // Prefer the field matching what was actually requested. Both fields
    // hold identical content today (`redact_text` sets `text` from the same
    // string as `markdown` on every ingest path), so this is a no-op in
    // practice right now, but it's the correct selection if that ever stops
    // being true.
    let output = if format == OutputFormat::Markdown {
        result.markdown.or(result.text)
    } else {
        result.text.or(result.markdown)
    };
    Ok(output.unwrap_or_default())
}

/// Redact PII from a plain image (jpg/png/…): OCR it, find PII via Tier A,
/// and black out each match's bounding box directly on the original
/// pixels. Returns the redacted image bytes, same format as the input.
/// Verified against a real photographed document (native build) — see the
/// repo README's "Build status" section; the wasm32 OCR path itself
/// (liteparse's pluggable OCR — bundled Tesseract is unavailable here, so
/// this needs `ocr_server_url` configured, which isn't exposed at this
/// binding layer yet) hasn't been separately verified in-browser.
///
/// No `redactPdf` here: liteparse's PDF-to-raster rendering
/// (`screenshot_input`) doesn't exist in its wasm32 build at all — a real
/// upstream constraint, not a choice made here (see `redact-paperasse-core`).
#[wasm_bindgen(js_name = redactImage)]
pub async fn redact_image(
    bytes: Vec<u8>,
    entities: Option<Vec<String>>,
    score_threshold: Option<f32>,
) -> Result<Vec<u8>, JsError> {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();

    let engine = Engine::default();
    let result = engine
        .process(
            Input::Image(bytes),
            OutputFormat::Native,
            entities.as_deref(),
            score_threshold,
        )
        .await
        .map_err(|e| JsError::new(&e.to_string()))?;
    result
        .bytes
        .ok_or_else(|| JsError::new("Engine::process returned no bytes for an Image input"))
}
