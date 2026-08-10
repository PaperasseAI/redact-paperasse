#![deny(clippy::all)]

use napi::bindgen_prelude::Buffer;
use napi_derive::napi;
use redact_paperasse_core::{Engine, Input, OutputFormat};

/// Options for `redactText`. All fields optional.
#[napi(object)]
pub struct RedactTextOptions {
    /// Structured markdown output is the default (this is a tool for
    /// agents, and markdown is what they parse best) — pass `false` to get
    /// back plain text in the input's own shape instead.
    pub markdown: Option<bool>,
    /// Only redact these entity types (e.g. `["FR_NIR"]`) — matches
    /// Presidio's `analyzer_entities` filter. Omit/undefined to redact
    /// every entity type Tier A's recognizers cover.
    pub entities: Option<Vec<String>>,
    /// Drop matches scoring below this (0.0-1.0). Matches Presidio's
    /// `score_threshold`. Omit/undefined for no filtering.
    pub score_threshold: Option<f64>,
}

/// Options for `redactImage`/`redactPdf`. Same filters as `RedactTextOptions`
/// minus `markdown` — these two always return pixel-redacted bytes (that's
/// the point of calling them specifically); OCR'd-and-redacted markdown
/// text from an image/PDF isn't exposed at this binding layer yet.
#[napi(object)]
pub struct RedactBytesOptions {
    /// Only redact these entity types (e.g. `["FR_NIR"]`) — matches
    /// Presidio's `analyzer_entities` filter. Omit/undefined to redact
    /// every entity type Tier A's recognizers cover.
    pub entities: Option<Vec<String>>,
    /// Drop matches scoring below this (0.0-1.0). Matches Presidio's
    /// `score_threshold`. Omit/undefined for no filtering.
    pub score_threshold: Option<f64>,
}

/// Redact PII from plain text — the fastest path (Tier A, in-process, no
/// network).
#[napi]
pub async fn redact_text(text: String, options: Option<RedactTextOptions>) -> napi::Result<String> {
    let options = options.unwrap_or(RedactTextOptions {
        markdown: None,
        entities: None,
        score_threshold: None,
    });
    let engine = Engine::default();
    let format = if options.markdown.unwrap_or(true) {
        OutputFormat::Markdown
    } else {
        OutputFormat::Native
    };
    let result = engine
        .process(
            Input::Text(text),
            format,
            options.entities.as_deref(),
            options.score_threshold.map(|t| t as f32),
        )
        .await
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
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

/// Redact PII from a plain image (jpg/png/…): OCR it (liteparse, bundled
/// Tesseract or a configured HTTP OCR server), find PII in the OCR'd text
/// via Tier A, and black out each match's bounding box directly on the
/// original pixels. Returns the redacted image bytes, same format as the
/// input. Verified against a real photographed document — see the repo
/// README's "Build status" section.
#[napi]
pub async fn redact_image(
    bytes: Buffer,
    options: Option<RedactBytesOptions>,
) -> napi::Result<Buffer> {
    let options = options.unwrap_or(RedactBytesOptions {
        entities: None,
        score_threshold: None,
    });
    let engine = Engine::default();
    let result = engine
        .process(
            Input::Image(bytes.to_vec()),
            OutputFormat::Native,
            options.entities.as_deref(),
            options.score_threshold.map(|t| t as f32),
        )
        .await
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    Ok(result
        .bytes
        .ok_or_else(|| {
            napi::Error::from_reason("Engine::process returned no bytes for an Image input")
        })?
        .into())
}

/// Redact PII from a PDF: render every page (liteparse/PDFium), find PII
/// via OCR + Tier A, black out matches, and reassemble a new PDF from the
/// redacted page images (`printpdf`). Deliberately flattens to an
/// image-based PDF — see `redact_pdf_bytes`'s doc comment in
/// `redact-paperasse-core` for why that's the correct behavior for
/// genuine redaction, not a limitation. Verified against a real document
/// embedded in a PDF — see the repo README's "Build status" section.
///
/// Not available in the WASM binding: liteparse's PDF-to-raster rendering
/// doesn't exist in its wasm32 build (an upstream constraint).
#[napi]
pub async fn redact_pdf(
    bytes: Buffer,
    options: Option<RedactBytesOptions>,
) -> napi::Result<Buffer> {
    let options = options.unwrap_or(RedactBytesOptions {
        entities: None,
        score_threshold: None,
    });
    let engine = Engine::default();
    let result = engine
        .process(
            Input::Pdf(bytes.to_vec()),
            OutputFormat::Native,
            options.entities.as_deref(),
            options.score_threshold.map(|t| t as f32),
        )
        .await
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    Ok(result
        .bytes
        .ok_or_else(|| {
            napi::Error::from_reason("Engine::process returned no bytes for a Pdf input")
        })?
        .into())
}
