use std::io::Cursor;

use image::{ImageReader, Rgba, RgbaImage};
#[cfg(not(target_arch = "wasm32"))]
use image::ImageFormat;
#[cfg(not(target_arch = "wasm32"))]
use liteparse::LiteParse;
#[cfg(not(target_arch = "wasm32"))]
use liteparse::types::PdfInput;
#[cfg(not(target_arch = "wasm32"))]
use printpdf::{Mm, Op, PdfDocument, PdfPage, PdfSaveOptions, RawImage, XObjectTransform};

use crate::error::EngineError;
use crate::types::{Entity, ExtractedDocument, OutputFormat, RedactionResult};

const MASK_CHAR: char = '█';
const MASK_COLOR: Rgba<u8> = Rgba([0, 0, 0, 255]);

/// Mask every detected entity's span, replacing it with a run of fixed-width
/// blocks (one per *character*, not per byte, so a masked span's visual
/// width roughly matches what it covered) — unlike Presidio's default
/// `<ENTITY_TYPE>` replacement, which shifts every later offset in the
/// string. Stability matters here because the same spans may also be used
/// to redact `word_boxes` on the same document.
///
/// Operates on byte offsets directly (via `text[cursor..start]`/
/// `text[start..end]` slicing) rather than collecting into a `Vec<char>` —
/// this was a real bug in an earlier version: a `Vec<char>` re-indexes the
/// whole string by character position, but `Entity::span` is always a byte
/// offset (from the `regex` crate), so any non-ASCII content BEFORE a match
/// silently shifted every mask out of position. Caught by the FR_NIR
/// recognizer's own "found within surrounding French text" test (the
/// accented context words shifted the match by 3 bytes vs. its char
/// count) — exactly the kind of input this is built for, so this couldn't
/// stay a "known limitation."
///
/// Match boundaries from a `regex` search over `text` are always valid
/// UTF-8 char boundaries, so slicing `text[start..end]` never panics.
/// Overlapping entities (e.g. Tier A and Tier B both matching over the same
/// span) are merged first so a masked run is never double-processed.
fn mask_text(text: &str, entities: &[Entity]) -> String {
    let mut ranges: Vec<(usize, usize)> = entities.iter().map(|e| (e.span.start, e.span.end)).collect();
    ranges.sort_unstable();

    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some((_, last_end)) if start <= *last_end => *last_end = (*last_end).max(end),
            _ => merged.push((start, end)),
        }
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for (start, end) in merged {
        out.push_str(&text[cursor..start]);
        for _ in 0..text[start..end].chars().count() {
            out.push(MASK_CHAR);
        }
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
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

/// Fill every entity's bounding box with `MASK_COLOR` on a decoded raster.
/// `page` filters to boxes on a specific page (for a rendered PDF page);
/// `None` draws every box regardless of page (a plain image has exactly
/// one implicit page). `dpi_scale` converts a box from the 72-DPI viewport
/// coordinates `WordBox`/`TextItem` use into the DPI the raster was
/// rendered/decoded at (`rendered_dpi / 72.0`).
fn draw_redaction_boxes(img: &mut RgbaImage, entities: &[Entity], page: Option<u32>, dpi_scale: f32) {
    let (img_w, img_h) = img.dimensions();
    for e in entities {
        let Some(bbox) = e.bbox else { continue };
        if let Some(p) = page {
            if bbox.page != p {
                continue;
            }
        }
        let x0 = (bbox.x * dpi_scale).max(0.0) as u32;
        let y0 = (bbox.y * dpi_scale).max(0.0) as u32;
        let x1 = (((bbox.x + bbox.width) * dpi_scale).max(0.0) as u32).min(img_w);
        let y1 = (((bbox.y + bbox.height) * dpi_scale).max(0.0) as u32).min(img_h);
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, MASK_COLOR);
            }
        }
    }
}

/// Redact a plain image (jpg/png/…): decode, black out every entity's
/// bounding box directly on the original pixels, re-encode in the same
/// format it came in.
///
/// Entities here carry boxes from the liteparse OCR ingest path
/// (`ingest::LiteparseIngestor::ingest_image`) — see that function's
/// build-validation note; the DPI this image was effectively "rendered" at
/// for OCR purposes needs confirming against the real crate, so
/// `assumed_dpi` defaults to liteparse's own default config DPI (150.0)
/// rather than the source image's own resolution, which is very likely
/// wrong for a camera photo. Flagged here rather than silently mispositioning
/// redaction boxes.
pub fn redact_image_bytes(bytes: &[u8], entities: &[Entity]) -> Result<Vec<u8>, EngineError> {
    let format =
        image::guess_format(bytes).map_err(|e| EngineError::Redact(format!("unrecognized image format: {e}")))?;
    let decoded = ImageReader::with_format(Cursor::new(bytes), format)
        .decode()
        .map_err(|e| EngineError::Redact(format!("failed to decode image: {e}")))?;

    let mut rgba = decoded.to_rgba8();
    let assumed_dpi = 150.0_f32; // TODO(build-validation): confirm against real ingest DPI.
    draw_redaction_boxes(&mut rgba, entities, None, assumed_dpi / 72.0);

    let mut out = Cursor::new(Vec::new());
    rgba.write_to(&mut out, format)
        .map_err(|e| EngineError::Redact(format!("failed to re-encode image: {e}")))?;
    Ok(out.into_inner())
}

/// Redact a PDF: render every page to a raster via liteparse (PDFium),
/// black out each entity's bounding box on the matching page, then
/// assemble a brand new PDF from the redacted page rasters (via
/// `printpdf`), one full-page image per page.
///
/// Deliberately flattens to an image-based PDF rather than trying to
/// selectively remove/cover the original vector text underneath a drawn
/// box — a black box drawn *over* a still-selectable text layer is a
/// well-known redaction failure mode (the "redacted" PII is still in the
/// file, just visually covered). Flattening removes the text layer
/// entirely, which is the correct behavior for genuine redaction, at the
/// cost of the output PDF no longer having selectable/searchable text.
///
/// KNOWN RISK: the `XObjectTransform`/page-sizing math that scales each
/// page image to exactly fill its `PdfPage` compiles against printpdf
/// 0.9.1's real API, but whether it's positioned/scaled *correctly* hasn't
/// been checked against an actual rendered PDF yet — see this crate's
/// README "Build status" section.
///
/// NOT AVAILABLE ON `wasm32`: depends on `LiteParse::screenshot_input`,
/// which liteparse itself only compiles for non-wasm32 targets (no
/// PDFium-to-raster rendering path in its wasm32 build). This is a real
/// upstream constraint, not a choice made here — pixel-level PDF redaction
/// structurally cannot exist in a browser build; only Tier A text
/// redaction can (see `bindings/wasm`, which only exposes the text path
/// for exactly this reason).
#[cfg(not(target_arch = "wasm32"))]
pub async fn redact_pdf_bytes(bytes: &[u8], entities: &[Entity], config: &liteparse::config::LiteParseConfig) -> Result<Vec<u8>, EngineError> {
    let parser = LiteParse::new(config.clone());
    let screenshots = parser
        .screenshot_input(PdfInput::Bytes(bytes.to_vec()), None)
        .await
        .map_err(|e| EngineError::Redact(format!("failed to render PDF pages: {e}")))?;

    let dpi_scale = config.dpi / 72.0;
    let mut doc = PdfDocument::new("redacted");
    let mut pages = Vec::with_capacity(screenshots.len());

    for shot in screenshots {
        let decoded = ImageReader::with_format(Cursor::new(&shot.image_bytes), ImageFormat::Png)
            .decode()
            .map_err(|e| EngineError::Redact(format!("failed to decode rendered page {}: {e}", shot.page_num)))?;
        let mut rgba = decoded.to_rgba8();
        draw_redaction_boxes(&mut rgba, entities, Some(shot.page_num), dpi_scale);

        let mut png_bytes = Cursor::new(Vec::new());
        rgba.write_to(&mut png_bytes, ImageFormat::Png)
            .map_err(|e| EngineError::Redact(format!("failed to re-encode redacted page {}: {e}", shot.page_num)))?;

        let raw_image = RawImage::decode_from_bytes(&png_bytes.into_inner(), &mut Vec::new())
            .map_err(|e| EngineError::Redact(format!("printpdf failed to load redacted page {}: {e}", shot.page_num)))?;
        let image_id = doc.add_image(&raw_image);

        let width_mm = Mm(shot.width as f32 / config.dpi * 25.4);
        let height_mm = Mm(shot.height as f32 / config.dpi * 25.4);
        let ops = vec![Op::UseXobject {
            id: image_id,
            transform: XObjectTransform {
                dpi: Some(config.dpi),
                ..Default::default()
            },
        }];
        pages.push(PdfPage::new(width_mm, height_mm, ops));
    }

    let mut warnings = Vec::new();
    Ok(doc.with_pages(pages).save(&PdfSaveOptions::default(), &mut warnings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DetectionSource, Span};

    fn entity(start: usize, end: usize) -> Entity {
        Entity {
            entity_type: "TEST".into(),
            span: Span { start, end },
            score: 1.0,
            bbox: None,
            source: DetectionSource::TierA,
        }
    }

    #[test]
    fn masks_correctly_with_multibyte_text_before_the_span() {
        // Regression test for the bug the FR_NIR recognizer's own test suite
        // caught: byte offsets from `regex`, char-indexed masking — wrong
        // whenever non-ASCII content (accented French, here) precedes the
        // match.
        let text = "mon numéro de sécurité sociale est 2 91 05 99 338 076 92";
        let matched = "2 91 05 99 338 076 92";
        let start = text.find(matched).expect("fixture contains the match");
        let redacted = mask_text(text, &[entity(start, start + matched.len())]);

        assert_eq!(redacted, "mon numéro de sécurité sociale est █████████████████████");
        assert!(!redacted.contains(matched));
        assert_eq!(redacted.chars().filter(|&c| c == MASK_CHAR).count(), matched.chars().count());
    }

    #[test]
    fn merges_overlapping_entities_instead_of_double_masking() {
        let redacted = mask_text("abcdefgh", &[entity(0, 4), entity(2, 6)]);
        assert_eq!(redacted, "██████gh");
    }

    #[test]
    fn leaves_untouched_text_around_the_span() {
        let redacted = mask_text("prefix SECRET suffix", &[entity(7, 13)]);
        assert_eq!(redacted, "prefix ██████ suffix");
    }
}
