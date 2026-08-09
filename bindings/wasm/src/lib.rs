//! Browser bindings. UNVERIFIED (see repo README's build-status note): both
//! `anydoc` and `liteparse` ship their own `wasm` binding crates, implying
//! their core crates are wasm32-compatible — but this crate depends on
//! `paperasse-privacy-core`, which pulls both in unconditionally, and that
//! combination hasn't been target-checked yet. Run
//! `cargo check --target wasm32-unknown-unknown -p paperasse-privacy-wasm`
//! once a toolchain is available and fix whatever it finds before relying
//! on this.

use paperasse_privacy_core::{Engine, Input, OutputFormat};
use wasm_bindgen::prelude::*;

/// Redact PII from plain text (Tier A: in-process regex+checksum
/// recognizers — the whole point of a WASM build, since it runs entirely
/// client-side with no server round trip). Pass `markdown: true` to force
/// markdown output.
#[wasm_bindgen(js_name = redactText)]
pub async fn redact_text(text: String, markdown: Option<bool>) -> Result<String, JsError> {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();

    let engine = Engine::default();
    let format = if markdown.unwrap_or(false) {
        OutputFormat::Markdown
    } else {
        OutputFormat::Native
    };
    let result = engine
        .process(Input::Text(text), format)
        .await
        .map_err(|e| JsError::new(&e.to_string()))?;
    Ok(result.text.or(result.markdown).unwrap_or_default())
}
