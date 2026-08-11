use std::io::Cursor;

use image::ImageFormat;
use image::{DynamicImage, ImageReader, Rgba, RgbaImage};
#[cfg(not(target_arch = "wasm32"))]
use liteparse::types::PdfInput;
#[cfg(not(target_arch = "wasm32"))]
use liteparse::LiteParse;
#[cfg(not(target_arch = "wasm32"))]
use printpdf::{
    ImageOptimizationOptions, Mm, Op, PdfDocument, PdfPage, PdfSaveOptions, RawImage,
    XObjectTransform,
};

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
/// Spans are sanitized before use: clamped to the text length and snapped
/// back to UTF-8 char boundaries. Tier A's regex matches never need this,
/// but Tier B's arrive from a network service (converted from char offsets
/// at that boundary), and a buggy or hostile analyzer must not be able to
/// panic the redactor -- an out-of-range span from a stub server did
/// exactly that ("end byte index 102 is out of bounds for string of length
/// 92") before this guard existed. Overlapping entities (e.g. Tier A and
/// Tier B both matching over the same span) are merged first so a masked
/// run is never double-processed.
fn mask_text(text: &str, entities: &[Entity]) -> String {
    let snap = |mut i: usize| -> usize {
        i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let mut ranges: Vec<(usize, usize)> = entities
        .iter()
        .map(|e| (snap(e.span.start), snap(e.span.end)))
        .filter(|(start, end)| start < end)
        .collect();
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
pub fn redact_text(
    doc: &ExtractedDocument,
    entities: &[Entity],
    format: OutputFormat,
) -> RedactionResult {
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
fn draw_redaction_boxes(
    img: &mut RgbaImage,
    entities: &[Entity],
    page: Option<u32>,
    dpi_scale: f32,
) {
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
pub fn redact_image_bytes(
    bytes: &[u8],
    entities: &[Entity],
    ingest_dpi: f32,
) -> Result<Vec<u8>, EngineError> {
    let format = image::guess_format(bytes)
        .map_err(|e| EngineError::Redact(format!("unrecognized image format: {e}")))?;
    let decoded = ImageReader::with_format(Cursor::new(bytes), format)
        .decode()
        .map_err(|e| EngineError::Redact(format!("failed to decode image: {e}")))?;

    let mut rgba = decoded.to_rgba8();
    // Must be the SAME dpi the ingest pass rasterized at: boxes come back in
    // 72-DPI viewport units and are scaled into this image's pixel space by
    // dpi/72. This used to be hardcoded to 150 while the engine's ingest DPI
    // was configurable, so raising the DPI to read finer print would have
    // silently misplaced every box. It's threaded through now.
    draw_redaction_boxes(&mut rgba, entities, None, ingest_dpi / 72.0);

    // Boxes are drawn on an RGBA buffer, but not every format can encode an
    // alpha channel — JPEG has none at all, and `image`'s encoder rejects
    // Rgba8 outright ("The encoder or decoder for Jpeg does not support the
    // color type `Rgba8`"). Found by a real upload of a photographed
    // document to the live demo: every JPEG failed, while the PNG fixtures
    // this was tested against all passed. Photos of paperwork are
    // overwhelmingly JPEG, so this was the main path being broken.
    //
    // Dropping alpha is lossless for our purposes: we decode an existing
    // image and draw fully opaque boxes, so nothing is ever transparent.
    // JPEG is handled up front as the known case; any other alpha-less
    // format (pnm, hdr, …) is caught by the retry rather than by trying to
    // enumerate the full list correctly.
    let mut out = Cursor::new(Vec::new());
    let write_result = if format == ImageFormat::Jpeg {
        DynamicImage::ImageRgba8(rgba.clone())
            .to_rgb8()
            .write_to(&mut out, format)
    } else {
        rgba.write_to(&mut out, format)
    };

    if let Err(err) = write_result {
        out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(rgba)
            .to_rgb8()
            .write_to(&mut out, format)
            .map_err(|_| {
                // Report the original RGBA error: the RGB retry failing too
                // means the format is unsupported for reasons that have
                // nothing to do with the alpha channel.
                EngineError::Redact(format!("failed to re-encode image: {err}"))
            })?;
    }

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
pub async fn redact_pdf_bytes(
    bytes: &[u8],
    entities: &[Entity],
    config: &liteparse::config::LiteParseConfig,
) -> Result<Vec<u8>, EngineError> {
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
            .map_err(|e| {
                EngineError::Redact(format!(
                    "failed to decode rendered page {}: {e}",
                    shot.page_num
                ))
            })?;
        let mut rgba = decoded.to_rgba8();
        draw_redaction_boxes(&mut rgba, entities, Some(shot.page_num), dpi_scale);

        let mut png_bytes = Cursor::new(Vec::new());
        rgba.write_to(&mut png_bytes, ImageFormat::Png)
            .map_err(|e| {
                EngineError::Redact(format!(
                    "failed to re-encode redacted page {}: {e}",
                    shot.page_num
                ))
            })?;

        let raw_image = RawImage::decode_from_bytes(&png_bytes.into_inner(), &mut Vec::new())
            .map_err(|e| {
                EngineError::Redact(format!(
                    "printpdf failed to load redacted page {}: {e}",
                    shot.page_num
                ))
            })?;
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

    // NOT PdfSaveOptions::default(): the default carries
    // image_optimization with a 2MB-per-image byte budget (quality 0.85,
    // auto-optimize on), which silently downscales embedded pages to fit.
    // On a real two-page Kbis that squeezed page 1 to an effective ~85 DPI
    // while page 2 kept ~147 -- the denser page compresses worse, so it got
    // shrunk harder -- and it made `--dpi` appear to do nothing for PDFs,
    // because whatever resolution came in, the optimizer converged it onto
    // the same budget. A redaction tool's output must carry exactly the
    // pixels the boxes were drawn on; growing the file is the honest cost.
    let save_options = PdfSaveOptions {
        image_optimization: Some(ImageOptimizationOptions {
            // The one non-negotiable: no byte budget, therefore no resizing.
            // Everything else about the optimizer is welcome -- alpha strip,
            // true-greyscale detection, JPEG at q0.90 -- because none of it
            // changes pixel dimensions. Fully lossless pages cost 13MB for a
            // two-page Kbis; q0.90 JPEG of a document render is visually
            // clean and two orders of magnitude smaller.
            max_image_size: None,
            quality: Some(0.90),
            // Dithering text edges helps photos, hurts glyphs.
            dither_greyscale: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut warnings = Vec::new();
    Ok(doc.with_pages(pages).save(&save_options, &mut warnings))
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
    fn an_out_of_range_span_is_clamped_rather_than_panicking() {
        // The stub-analyzer poison test found this as a real panic: span
        // 102..112 against 92 bytes of text took down the process. A span
        // no text backs redacts nothing -- and the placement guard in
        // process_with_tier_b is what decides whether that miss is an
        // error; masking's only job is to never crash.
        let text = "short text";
        assert_eq!(mask_text(text, &[entity(102, 112)]), "short text");
    }

    #[test]
    fn a_span_cutting_a_multibyte_char_snaps_to_the_boundary() {
        // é spans bytes 3..5; ending at byte 4 would slice mid-char.
        let masked = mask_text("Numéro", &[entity(0, 4)]);
        assert!(!masked.is_empty(), "must not panic and must produce text");
    }

    #[test]
    fn masks_correctly_with_multibyte_text_before_the_span() {
        // Regression test for the bug the FR_NIR recognizer's own test suite
        // caught: byte offsets from `regex`, char-indexed masking — wrong
        // whenever non-ASCII content (accented French, here) precedes the
        // match.
        let text = "mon numéro de sécurité sociale est 1 85 01 75 123 456 09";
        let matched = "1 85 01 75 123 456 09";
        let start = text.find(matched).expect("fixture contains the match");
        let redacted = mask_text(text, &[entity(start, start + matched.len())]);

        assert_eq!(
            redacted,
            "mon numéro de sécurité sociale est █████████████████████"
        );
        assert!(!redacted.contains(matched));
        assert_eq!(
            redacted.chars().filter(|&c| c == MASK_CHAR).count(),
            matched.chars().count()
        );
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

    /// Encode a small solid-white image in `format`, entirely in memory —
    /// no fixture files, so this stays a real unit test.
    fn encode_blank(format: ImageFormat) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            64,
            image::Rgb([255, 255, 255]),
        ));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, format).expect("encode should work");
        buf.into_inner()
    }

    fn entity_with_box() -> Entity {
        Entity {
            entity_type: "TEST".into(),
            span: Span { start: 0, end: 4 },
            score: 1.0,
            bbox: Some(crate::types::BoundingBox {
                page: 1,
                x: 4.0,
                y: 4.0,
                width: 8.0,
                height: 4.0,
            }),
            source: DetectionSource::TierA,
        }
    }

    #[test]
    fn redacts_a_jpeg_without_failing_on_the_alpha_channel() {
        // Regression test for a real bug found by a real upload to the live
        // demo, not by any fixture here: redaction draws boxes on an RGBA
        // buffer, but JPEG has no alpha channel and `image`'s encoder
        // rejects Rgba8 outright. Every JPEG upload failed while every PNG
        // passed — and photographed paperwork, the whole point of this
        // tool, is overwhelmingly JPEG.
        let jpeg = encode_blank(ImageFormat::Jpeg);
        let out = redact_image_bytes(&jpeg, &[entity_with_box()], 150.0)
            .expect("redacting a JPEG must not fail on the alpha channel");

        assert_eq!(
            image::guess_format(&out).expect("output should be a real image"),
            ImageFormat::Jpeg,
            "a JPEG in should stay a JPEG out",
        );
        // And it must still decode — an encoder that "succeeded" into
        // garbage bytes would be worse than the original error.
        image::load_from_memory(&out).expect("redacted JPEG should decode");
    }

    #[test]
    fn redacts_a_png_preserving_its_format() {
        // The case that always worked — kept so a future fix for one format
        // can't silently break the other.
        let png = encode_blank(ImageFormat::Png);
        let out =
            redact_image_bytes(&png, &[entity_with_box()], 150.0).expect("redacting a PNG works");
        assert_eq!(
            image::guess_format(&out).expect("output should be a real image"),
            ImageFormat::Png,
        );
        image::load_from_memory(&out).expect("redacted PNG should decode");
    }
}
