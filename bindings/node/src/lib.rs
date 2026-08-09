#![deny(clippy::all)]

use napi_derive::napi;
use paperasse_privacy_core::{Engine, Input, OutputFormat};

/// Redact PII from plain text — the fastest path (Tier A, in-process, no
/// network). Set `markdown: true` to force markdown output.
///
/// napi-rs converts this to `redactText(text, markdown?)` in JS.
///
/// Image/PDF bindings land once `redact::redact_image` in the core crate is
/// implemented (currently a stub — see that crate's TODO).
#[napi]
pub async fn redact_text(text: String, markdown: Option<bool>) -> napi::Result<String> {
    let engine = Engine::default();
    let format = if markdown.unwrap_or(false) {
        OutputFormat::Markdown
    } else {
        OutputFormat::Native
    };
    let result = engine
        .process(Input::Text(text), format)
        .await
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    Ok(result.text.or(result.markdown).unwrap_or_default())
}
