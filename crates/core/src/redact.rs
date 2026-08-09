use crate::error::EngineError;
use crate::types::{Entity, ExtractedDocument, OutputFormat, RedactionResult};

const MASK_CHAR: char = '█';

/// Mask every detected entity's span, replacing each character with a
/// fixed-width block so offsets/lengths stay stable — unlike Presidio's
/// default `<ENTITY_TYPE>` replacement, which shifts every later offset in
/// the string. Stability matters here because the same spans may also be
/// used to redact `word_boxes` on the same document.
///
/// KNOWN LIMITATION (v0.1): spans are byte offsets from the recognizer
/// (`regex` crate, UTF-8-boundary-safe), but this masks by `char` index —
/// correct for ASCII spans (which covers every Tier A recognizer today:
/// digits, `@`, `.`), but will misalign on a span that starts/ends inside a
/// multi-byte character run. Fix by masking on the byte slice directly (via
/// `String::replace_range` per span) once this is under a real build/test
/// loop — flagging rather than silently shipping wrong for non-ASCII PII.
fn mask_text(text: &str, entities: &[Entity]) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    for e in entities {
        for c in chars.iter_mut().take(e.span.end).skip(e.span.start) {
            *c = MASK_CHAR;
        }
    }
    chars.into_iter().collect()
}

/// Redact a text or markdown document. Used for `Input::Text`, for any
/// document ingested without layout info (the anydoc path), and whenever
/// `OutputFormat::Markdown` is requested regardless of the original input
/// type.
pub fn redact_text(doc: &ExtractedDocument, entities: &[Entity], format: OutputFormat) -> RedactionResult {
    let redacted_text = mask_text(&doc.text, entities);
    let redacted_markdown = doc.markdown.as_deref().map(|md| mask_text(md, entities));

    match format {
        OutputFormat::Markdown => RedactionResult {
            format,
            markdown: Some(redacted_markdown.unwrap_or_else(|| redacted_text.clone())),
            text: Some(redacted_text),
            bytes: None,
            entities: entities.to_vec(),
        },
        OutputFormat::Native => RedactionResult {
            format,
            text: Some(redacted_text),
            markdown: redacted_markdown,
            bytes: None,
            entities: entities.to_vec(),
        },
    }
}

/// Draw a filled box over each entity's bounding box on the source
/// image/PDF and re-encode. NOT YET IMPLEMENTED — this is the one piece of
/// the pipeline that still needs real work:
///
/// 1. Render each affected page to pixels (liteparse's screenshot API, or
///    PDFium directly for a PDF; pass through for a plain image).
/// 2. For each entity with a `bbox`, fill that rect (the `image` crate's
///    `imageops`/`draw` helpers) on the matching page's raster.
/// 3. For a PDF input, re-composite the redacted page rasters back into a
///    PDF (or return page images — decide the contract before implementing).
///
/// Wired through `Engine::process` already so the pipeline's shape is
/// settled; only this function's body is a stub.
pub fn redact_image(_bytes: &[u8], _entities: &[Entity]) -> Result<Vec<u8>, EngineError> {
    Err(EngineError::Unsupported(
        "image/PDF pixel redaction not yet implemented — see redact::redact_image".into(),
    ))
}
