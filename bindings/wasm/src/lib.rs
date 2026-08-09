//! Browser bindings. Verified against the real target:
//! `cargo check --target wasm32-unknown-unknown -p paperasse-privacy-wasm`
//! passes clean (zero warnings) — see the repo README's "Build status" for
//! what that took (a Cargo workspace-inheritance fix so liteparse's
//! `tesseract` feature, which doesn't target wasm32, is off by default
//! here; `redact_pdf_bytes` is `#[cfg(not(target_arch = "wasm32"))]`-gated
//! in the core crate since liteparse's own `screenshot_input` doesn't exist
//! on wasm32 either — a real upstream constraint, not a choice made here).
//! This binding therefore only exposes Tier A text redaction, which is
//! exactly what runs client-side in a browser anyway.

use paperasse_privacy_core::{Engine, Input, OutputFormat};
use wasm_bindgen::prelude::*;

/// Redact PII from plain text (Tier A: in-process regex+checksum
/// recognizers — the whole point of a WASM build, since it runs entirely
/// client-side with no server round trip). Pass `markdown: true` to force
/// markdown output. Pass `entities: ["FR_NIR"]` to redact only that entity
/// type — matches Presidio's `analyzer_entities` filter; omit/`undefined`
/// to redact every entity type Tier A's recognizers cover.
#[wasm_bindgen(js_name = redactText)]
pub async fn redact_text(
    text: String,
    markdown: Option<bool>,
    entities: Option<Vec<String>>,
) -> Result<String, JsError> {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();

    let engine = Engine::default();
    let format = if markdown.unwrap_or(false) {
        OutputFormat::Markdown
    } else {
        OutputFormat::Native
    };
    let result = engine
        .process(Input::Text(text), format, entities.as_deref())
        .await
        .map_err(|e| JsError::new(&e.to_string()))?;
    Ok(result.text.or(result.markdown).unwrap_or_default())
}
