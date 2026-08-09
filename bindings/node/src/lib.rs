#![deny(clippy::all)]

use napi_derive::napi;
use paperasse_privacy_core::{Engine, Input, OutputFormat};

/// Options for `redactText`. All fields optional.
#[napi(object)]
pub struct RedactTextOptions {
    /// Force structured markdown output instead of plain text. Default false.
    pub markdown: Option<bool>,
    /// Only redact these entity types (e.g. `["FR_NIR"]`) — matches
    /// Presidio's `analyzer_entities` filter. Omit/undefined to redact
    /// every entity type Tier A's recognizers cover.
    pub entities: Option<Vec<String>>,
}

/// Redact PII from plain text — the fastest path (Tier A, in-process, no
/// network). Image/PDF redaction (`redactImage`/`redactPdf`) is planned but
/// not yet exposed at this binding layer; the underlying pipeline
/// (`paperasse-privacy-core`'s `redact_image_bytes`/`redact_pdf_bytes`) is
/// implemented and tested — see that crate.
#[napi]
pub async fn redact_text(text: String, options: Option<RedactTextOptions>) -> napi::Result<String> {
    let options = options.unwrap_or(RedactTextOptions {
        markdown: None,
        entities: None,
    });
    let engine = Engine::default();
    let format = if options.markdown.unwrap_or(false) {
        OutputFormat::Markdown
    } else {
        OutputFormat::Native
    };
    let result = engine
        .process(Input::Text(text), format, options.entities.as_deref())
        .await
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    Ok(result.text.or(result.markdown).unwrap_or_default())
}
