#![deny(clippy::all)]

use napi::bindgen_prelude::Buffer;
use napi_derive::napi;
use redact_paperasse_core::{Engine, Input, OutputFormat};

/// Presidio NER pass configuration — the opt-in "Tier B". When present,
/// detection runs the local recognizers *and* a Presidio `/analyze` call,
/// with Presidio's spans routed through the same OCR word boxes the local
/// recognizers use, so a PERSON found on a photographed letter gets a real
/// black box on the image.
///
/// Two behaviours worth knowing before turning it on:
///
/// * **Fail-closed, twice.** If the analyzer is unreachable, redaction
///   errors rather than quietly returning a result missing every name it
///   was asked to catch. And if a Presidio span can't be matched to any
///   OCR word box on a pixel output, the whole run errors naming the
///   entity types that would have been silently visible.
/// * **Your text goes to that URL.** Point this at an analyzer *you* run
///   (localhost or your own network). Nothing in this library ever picks a
///   remote endpoint on its own.
#[napi(object)]
pub struct TierBOptions {
    /// Base URL of a running presidio-analyzer, e.g. `http://localhost:5002`.
    pub analyzer_url: String,
    /// Language code Presidio should analyze with (e.g. `"fr"`, `"en"`).
    /// Defaults to `"en"` — the stock analyzer image only loads English;
    /// French needs an analyzer configured with a French NLP model.
    pub language: Option<String>,
}

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
    /// Run a Presidio NER pass alongside the local recognizers — see
    /// `TierBOptions` for the two fail-closed behaviours this brings.
    pub tier_b: Option<TierBOptions>,
}

/// Options for `redactImage`/`redactPdf`/`redactImageText`/`redactPdfText`.
/// Same filters as `RedactTextOptions` minus `markdown`: `redactImage`/
/// `redactPdf` always return pixel-redacted bytes (no markdown/native
/// choice — that's the point of calling them specifically), and
/// `redactImageText`/`redactPdfText` always return OCR'd redacted text
/// (there's no meaningful "native" alternative to toggle to for those --
/// `OutputFormat::Native` means pixel bytes for an image/PDF input, not
/// plain text, so a `markdown` flag here would be misleading).
#[napi(object)]
pub struct RedactBytesOptions {
    /// Only redact these entity types (e.g. `["FR_NIR"]`) — matches
    /// Presidio's `analyzer_entities` filter. Omit/undefined to redact
    /// every entity type Tier A's recognizers cover.
    pub entities: Option<Vec<String>>,
    /// Drop matches scoring below this (0.0-1.0). Matches Presidio's
    /// `score_threshold`. Omit/undefined for no filtering.
    pub score_threshold: Option<f64>,
    /// Run a Presidio NER pass alongside the local recognizers — see
    /// `TierBOptions` for the two fail-closed behaviours this brings.
    pub tier_b: Option<TierBOptions>,
}

/// Run the engine with or without the Presidio pass, depending on options.
/// One function so the five public entry points cannot drift apart on how
/// Tier B is wired.
async fn run_engine(
    input: Input,
    format: OutputFormat,
    entities: Option<&[String]>,
    score_threshold: Option<f32>,
    tier_b: Option<&TierBOptions>,
) -> napi::Result<redact_paperasse_core::RedactionResult> {
    let engine = Engine::default();
    let result = match tier_b {
        Some(tb) => {
            let client = redact_paperasse_core::detect::TierB::new(tb.analyzer_url.clone());
            engine
                .process_with_tier_b(
                    input,
                    format,
                    entities,
                    score_threshold,
                    &client,
                    tb.language.as_deref().unwrap_or("en"),
                )
                .await
        }
        None => {
            engine
                .process(input, format, entities, score_threshold)
                .await
        }
    };
    result.map_err(|e| napi::Error::from_reason(e.to_string()))
}

/// Redact PII from plain text — the fastest path (Tier A, in-process, no
/// network).
#[napi]
pub async fn redact_text(text: String, options: Option<RedactTextOptions>) -> napi::Result<String> {
    let options = options.unwrap_or(RedactTextOptions {
        markdown: None,
        entities: None,
        score_threshold: None,
        tier_b: None,
    });
    let format = if options.markdown.unwrap_or(true) {
        OutputFormat::Markdown
    } else {
        OutputFormat::Native
    };
    let result = run_engine(
        Input::Text(text),
        format,
        options.entities.as_deref(),
        options.score_threshold.map(|t| t as f32),
        options.tier_b.as_ref(),
    )
    .await?;
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
        tier_b: None,
    });
    let result = run_engine(
        Input::Image(bytes.to_vec()),
        OutputFormat::Native,
        options.entities.as_deref(),
        options.score_threshold.map(|t| t as f32),
        options.tier_b.as_ref(),
    )
    .await?;
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
        tier_b: None,
    });
    let result = run_engine(
        Input::Pdf(bytes.to_vec()),
        OutputFormat::Native,
        options.entities.as_deref(),
        options.score_threshold.map(|t| t as f32),
        options.tier_b.as_ref(),
    )
    .await?;
    Ok(result
        .bytes
        .ok_or_else(|| {
            napi::Error::from_reason("Engine::process returned no bytes for a Pdf input")
        })?
        .into())
}

/// Redact PII from a plain image (jpg/png/…) and return the OCR'd redacted
/// text directly — no pixel step. `Engine::process`'s `OutputFormat::Markdown`
/// branch runs before the input-type match, so this OCRs via liteparse,
/// finds PII in the OCR'd text via Tier A, and returns the redacted result
/// as text, skipping the bounding-box/pixel-drawing work `redactImage` does.
/// Cheaper than `redactImage` when an agent only needs the text content,
/// not a redacted image to display.
#[napi]
pub async fn redact_image_text(
    bytes: Buffer,
    options: Option<RedactBytesOptions>,
) -> napi::Result<String> {
    let options = options.unwrap_or(RedactBytesOptions {
        entities: None,
        score_threshold: None,
        tier_b: None,
    });
    let result = run_engine(
        Input::Image(bytes.to_vec()),
        OutputFormat::Markdown,
        options.entities.as_deref(),
        options.score_threshold.map(|t| t as f32),
        options.tier_b.as_ref(),
    )
    .await?;
    Ok(result.markdown.or(result.text).unwrap_or_default())
}

/// Redact PII from a PDF and return the OCR'd redacted text directly — no
/// pixel step, no page-image reassembly. Same reasoning as
/// `redactImageText`: `OutputFormat::Markdown` short-circuits
/// `Engine::process` before it reaches the pixel-redaction path.
///
/// Not available in the WASM binding, same constraint as `redactPdf`.
#[napi]
pub async fn redact_pdf_text(
    bytes: Buffer,
    options: Option<RedactBytesOptions>,
) -> napi::Result<String> {
    let options = options.unwrap_or(RedactBytesOptions {
        entities: None,
        score_threshold: None,
        tier_b: None,
    });
    let result = run_engine(
        Input::Pdf(bytes.to_vec()),
        OutputFormat::Markdown,
        options.entities.as_deref(),
        options.score_threshold.map(|t| t as f32),
        options.tier_b.as_ref(),
    )
    .await?;
    Ok(result.markdown.or(result.text).unwrap_or_default())
}
