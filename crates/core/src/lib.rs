//! `redact-paperasse-core`: image/PDF/text in, redacted image/PDF/text/markdown
//! out. A privacy engine built for agents — fast enough to run in-process
//! (via the Node/Python/WASM bindings in this workspace) rather than only as
//! a REST call.
//!
//! ```text
//! Input (image | pdf | text)
//!    │
//!    ▼
//! [1] ingest   — anydoc (office formats + text PDFs) or liteparse
//!                (scanned/image PDFs, plain images — OCR + bounding boxes)
//!    │
//!    ▼
//! [2] detect   — Tier A (in-process regex+checksum, default) and/or
//!                Tier B (optional REST call to Presidio for NER)
//!    │
//!    ▼
//! [3] redact   — mask text/markdown spans, or fill pixel bounding boxes
//!    │
//!    ▼
//! Output (redacted image | redacted pdf | redacted text | markdown)
//! ```
//!
//! All three stages are wired end to end, including pixel redaction
//! (`redact::redact_image_bytes` / `redact::redact_pdf_bytes`) — see those
//! functions' doc comments for what's still unverified against a real
//! build (DPI assumptions, the printpdf reassembly transform).

pub mod detect;
pub mod error;
pub mod ingest;
pub mod redact;
pub mod types;

pub use error::EngineError;
pub use types::{
    DetectionSource, DocumentFormat, Entity, ExtractedDocument, Input, OutputFormat,
    RedactionResult,
};

use detect::TierA;
use ingest::{DefaultIngestor, Ingestor};

/// The default pipeline: ingest → Tier A detect → redact. Tier B (Presidio)
/// is opt-in — construct `detect::TierB` separately, merge its results with
/// Tier A's, and call the `redact` module's functions directly when broader
/// NER coverage is worth the extra hop than this convenience wrapper
/// provides.
/// Default NxN tile grid for the supplementary image OCR pass. 2 keeps the
/// added latency to roughly one extra full-image pass while already
/// recovering the small-print case that motivated it; 1 disables it.
pub const DEFAULT_OCR_TILING: u32 = 2;

/// Merge `extra` into `found`, dropping anything whose box substantially
/// overlaps a box already present for the same entity type. The tiled pass
/// re-reads overlap regions by design, so the same identifier legitimately
/// arrives more than once; without this the report would list it twice.
/// Drawing is idempotent either way -- this is about not lying in
/// `--report` about how many distinct things were found.
fn merge_by_box(found: &mut Vec<Entity>, extra: Vec<Entity>) {
    for e in extra {
        let Some(nb) = e.bbox else { continue };
        let dup = found.iter().any(|f| match f.bbox {
            Some(b) => {
                f.entity_type == e.entity_type
                    && b.page == nb.page
                    // Centre of one inside the other is a robust enough
                    // test here: tiles shift a box by sub-pixel rounding,
                    // never by a whole field.
                    && (nb.x + nb.width / 2.0) >= b.x
                    && (nb.x + nb.width / 2.0) <= b.x + b.width
                    && (nb.y + nb.height / 2.0) >= b.y
                    && (nb.y + nb.height / 2.0) <= b.y + b.height
            }
            None => false,
        });
        if !dup {
            found.push(e);
        }
    }
}

pub struct Engine {
    ingestor: Box<dyn Ingestor>,
    tier_a: TierA,
    // Kept alongside `ingestor` (rather than reached through it) because
    // `redact_pdf_bytes` needs the same DPI the ingest pass used, and
    // `Ingestor` is a trait object with no way to expose that. Always built
    // together with whatever `DefaultIngestor`'s own internal
    // `LiteparseIngestor` uses — see `with_liteparse_config`, the only
    // place a caller can set this, which threads one config through both
    // instead of the two being independently constructed.
    liteparse_config: liteparse::config::LiteParseConfig,
    /// NxN overlapping-tile OCR grid for images; 1 disables the extra pass.
    ocr_tiling: u32,
}

impl Default for Engine {
    fn default() -> Self {
        Self::with_liteparse_config(liteparse::config::LiteParseConfig::default())
    }
}

impl Engine {
    /// Build an engine with a specific ingestor — e.g.
    /// `Engine::new(Box::new(ingest::AnydocIngestor))` to force the
    /// lightweight, no-native-binary path when the caller already knows
    /// none of its documents are scans. Uses a default `LiteParseConfig`
    /// for `redact_pdf_bytes`'s DPI — use `with_liteparse_config` instead
    /// if the ingestor you're passing in was built with a non-default one.
    pub fn new(ingestor: Box<dyn Ingestor>) -> Self {
        Self {
            ingestor,
            tier_a: TierA::default(),
            liteparse_config: liteparse::config::LiteParseConfig::default(),
            ocr_tiling: DEFAULT_OCR_TILING,
        }
    }

    /// Build the default pipeline with a custom `LiteParseConfig` (e.g. a
    /// non-default `dpi`, or an `ocr_server_url` pointing at an external
    /// OCR service instead of bundled Tesseract) — used consistently by
    /// both the ingest pass (`DefaultIngestor`'s internal
    /// `LiteparseIngestor`) and the redact pass (`redact_pdf_bytes`), so
    /// e.g. a custom DPI can't silently mismatch between the two.
    pub fn with_liteparse_config(config: liteparse::config::LiteParseConfig) -> Self {
        Self {
            ingestor: Box::new(DefaultIngestor::with_liteparse_config(config.clone())),
            tier_a: TierA::default(),
            liteparse_config: config,
            ocr_tiling: DEFAULT_OCR_TILING,
        }
    }

    /// Run the default pipeline. `format` controls the output shape:
    /// `OutputFormat::Native` mirrors the input's own type (redacted image
    /// stays an image, redacted PDF stays a PDF); `OutputFormat::Markdown`
    /// forces structured markdown output regardless of input type.
    ///
    /// `entities` mirrors Presidio's `analyzer_entities` filter: `None`
    /// redacts everything Tier A can find; `Some(&["FR_NIR".into()])`
    /// redacts only that entity type and leaves everything else (an email,
    /// say) untouched. `score_threshold` mirrors Presidio's own field of
    /// the same name: a match scoring below it is dropped. See
    /// `detect::TierA::analyze`'s doc comment for both.
    pub async fn process(
        &self,
        input: Input,
        format: OutputFormat,
        entities: Option<&[String]>,
        score_threshold: Option<f32>,
    ) -> Result<RedactionResult, EngineError> {
        // A Document input (DOCX/XLSX/PPTX/...) only ever converts TO
        // markdown (anydoc never writes back to a native office format), so
        // there's no "redacted native document" this pipeline can produce.
        // Reject up front rather than doing an ingest pass just to throw it
        // away, or worse, silently falling back to something unintended.
        if format == OutputFormat::Native && matches!(input, Input::Document { .. }) {
            return Err(EngineError::Unsupported(
                "OutputFormat::Native has no meaning for a Document input — anydoc only converts \
                 to markdown, never back to DOCX/XLSX/etc.; request OutputFormat::Markdown instead"
                    .into(),
            ));
        }

        // Only a Native-format Pdf/Image needs pixel coordinates out of
        // ingestion — Text and Document never do, and Markdown output never
        // does regardless of input type (see `Ingestor::ingest`'s doc
        // comment). Matched positively (only these two need it) rather than
        // negated, so a future Input variant defaults to NOT requesting
        // boxes instead of silently requesting them.
        let needs_boxes =
            format == OutputFormat::Native && matches!(input, Input::Pdf(_) | Input::Image(_));
        // Correct EXIF orientation ONCE, up front, so OCR and the pixels we
        // draw boxes on are the same image. Doing it here rather than inside
        // redact_image_bytes matters: if only the redaction step rotated,
        // the boxes would be placed using coordinates from a differently
        // oriented OCR pass and land in the wrong place.
        let mut input = match input {
            Input::Image(bytes) => {
                Input::Image(ingest::normalize_orientation(&bytes).unwrap_or(bytes))
            }
            other => other,
        };

        let mut doc = self.ingestor.ingest(&input, needs_boxes).await?;
        let mut found = self.tier_a.analyze(&doc, entities, score_threshold);

        // If an image turned up nothing, try the other three orientations
        // before concluding there's nothing to redact. An EXIF tag only
        // helps when the file carries one, and messaging apps strip them,
        // scanners often don't write them, and screenshots have none -- so
        // a page can be stored sideways with nothing to say so. The person
        // uploading it sees it upright in their gallery and has no reason
        // to think it needs rotating, so we cannot make that their job.
        //
        // Only runs on the empty result, so the common upright case pays
        // nothing. Whichever orientation actually reads becomes the image
        // we redact and return, which also hands back an upright file.
        if found.is_empty() && self.ocr_tiling > 0 {
            if let Input::Image(ref bytes) = input {
                for degrees in [90u32, 180, 270] {
                    let Some(rotated) = ingest::rotate_bytes(bytes, degrees) else {
                        continue;
                    };
                    let candidate = Input::Image(rotated.clone());
                    let Ok(rdoc) = self.ingestor.ingest(&candidate, needs_boxes).await else {
                        continue;
                    };
                    let rfound = self.tier_a.analyze(&rdoc, entities, score_threshold);
                    if !rfound.is_empty() {
                        input = candidate;
                        doc = rdoc;
                        found = rfound;
                        break;
                    }
                }
            }
        }

        // A single full-page OCR pass reliably reads large print and can
        // miss small print entirely -- not because of resolution, but
        // because Tesseract's page segmentation never isolates a dense
        // little block on a busy page, so that text is never produced at
        // all. Verified on a real scanned letter carrying the same NIR
        // twice: the full page found 1 of 2, while the identical pixels
        // cropped found the missing one every time.
        //
        // So for pixel redaction, OCR the image a second time in
        // overlapping tiles and merge. Missing PII is the worst failure
        // this tool has, which is what justifies the extra pass; it only
        // runs when boxes are actually needed (Native output for an image),
        // never on the text/markdown path.
        if needs_boxes && self.ocr_tiling > 1 {
            if let Input::Image(ref bytes) = input {
                let tiler = ingest::LiteparseIngestor::with_config(self.liteparse_config.clone());
                if let Ok(tiled) = tiler.ingest_image_tiled(bytes, self.ocr_tiling, 0.15).await {
                    let extra = self.tier_a.analyze(&tiled, entities, score_threshold);
                    merge_by_box(&mut found, extra);
                }
            }
        }
        let entities = found;

        if format == OutputFormat::Markdown {
            return Ok(redact::redact_text(&doc, &entities, format));
        }

        match input {
            Input::Text(_) | Input::Document { .. } => {
                Ok(redact::redact_text(&doc, &entities, format))
            }
            Input::Image(bytes) => {
                let redacted_bytes =
                    redact::redact_image_bytes(&bytes, &entities, crate::ingest::IMAGE_OCR_DPI)?;
                Ok(RedactionResult {
                    format,
                    bytes: Some(redacted_bytes),
                    entities,
                    ..Default::default()
                })
            }
            #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
            Input::Pdf(bytes) => {
                // Pixel-level PDF redaction needs LiteParse::screenshot_input,
                // which doesn't exist in liteparse's own wasm32 build (no
                // PDFium-to-raster path there) — an upstream constraint, not
                // a choice made here. See `redact::redact_pdf_bytes`'s doc
                // comment.
                #[cfg(target_arch = "wasm32")]
                {
                    let _ = &self.liteparse_config; // silence unused-field warning on this target
                    Err(EngineError::Unsupported(
                        "pixel-level PDF redaction is unavailable in a wasm32 build — request \
                         OutputFormat::Markdown instead, or use a native/Node/Python build for \
                         OutputFormat::Native"
                            .into(),
                    ))
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let redacted_bytes =
                        redact::redact_pdf_bytes(&bytes, &entities, &self.liteparse_config).await?;
                    Ok(RedactionResult {
                        format,
                        bytes: Some(redacted_bytes),
                        entities,
                        ..Default::default()
                    })
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BoundingBox, DetectionSource, Span};

    fn boxed(entity_type: &str, x: f32, y: f32) -> Entity {
        Entity {
            entity_type: entity_type.into(),
            span: Span { start: 0, end: 4 },
            score: 1.0,
            bbox: Some(BoundingBox {
                page: 1,
                x,
                y,
                width: 20.0,
                height: 10.0,
            }),
            source: DetectionSource::TierA,
        }
    }

    #[test]
    fn merge_keeps_a_genuinely_separate_second_occurrence() {
        // The case this whole tiled pass exists for: the same NIR printed
        // twice on one page, far apart. Both must survive the merge.
        let mut found = vec![boxed("FR_NIR", 100.0, 100.0)];
        merge_by_box(&mut found, vec![boxed("FR_NIR", 100.0, 4000.0)]);
        assert_eq!(found.len(), 2, "a second, distant occurrence must be kept");
    }

    #[test]
    fn merge_drops_the_same_box_seen_twice_across_overlapping_tiles() {
        // Tiles overlap on purpose, so the same identifier legitimately
        // arrives twice with near-identical boxes. Reporting it twice would
        // overstate what was found.
        let mut found = vec![boxed("FR_NIR", 100.0, 100.0)];
        merge_by_box(&mut found, vec![boxed("FR_NIR", 101.0, 101.0)]);
        assert_eq!(found.len(), 1, "an overlapping duplicate must be dropped");
    }

    #[test]
    fn merge_keeps_a_different_entity_type_at_the_same_spot() {
        let mut found = vec![boxed("FR_NIR", 100.0, 100.0)];
        merge_by_box(&mut found, vec![boxed("IBAN_CODE", 100.0, 100.0)]);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn merge_ignores_boxless_entities() {
        // Tier B has no coordinates; it must not silently contribute an
        // undrawable entity to a pixel-redaction result.
        let mut found = vec![boxed("FR_NIR", 100.0, 100.0)];
        let mut boxless = boxed("PERSON", 0.0, 0.0);
        boxless.bbox = None;
        merge_by_box(&mut found, vec![boxless]);
        assert_eq!(found.len(), 1);
    }

    #[tokio::test]
    async fn redacts_fr_nir_in_plain_text() {
        let engine = Engine::default();
        let result = engine
            .process(
                Input::Text("mon NIR est 185017512345609, merci.".to_string()),
                OutputFormat::Native,
                None,
                None,
            )
            .await
            .expect("text pipeline never hits the unimplemented pixel path");

        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].entity_type, "FR_NIR");
        assert!(!result.text.unwrap().contains("185017512345609"));
    }

    #[tokio::test]
    async fn entities_filter_redacts_only_the_requested_type() {
        let engine = Engine::default();
        let text = "email: john@example.com, nir: 185017512345609".to_string();

        let filtered = engine
            .process(
                Input::Text(text.clone()),
                OutputFormat::Native,
                Some(&["FR_NIR".to_string()]),
                None,
            )
            .await
            .unwrap();
        assert_eq!(filtered.entities.len(), 1);
        assert_eq!(filtered.entities[0].entity_type, "FR_NIR");
        let redacted = filtered.text.unwrap();
        assert!(redacted.contains("john@example.com")); // untouched — not in the filter
        assert!(!redacted.contains("185017512345609"));

        let unfiltered = engine
            .process(Input::Text(text), OutputFormat::Native, None, None)
            .await
            .unwrap();
        assert_eq!(unfiltered.entities.len(), 2); // both FR_NIR and EMAIL_ADDRESS
    }

    #[tokio::test]
    async fn score_threshold_drops_low_confidence_matches() {
        let engine = Engine::default();
        // Email always scores 0.9, FR_NIR always scores 1.0 (see TierA::analyze's
        // doc comment) — a 0.95 threshold should keep the NIR and drop the email.
        let text = "email: john@example.com, nir: 185017512345609".to_string();

        let result = engine
            .process(Input::Text(text), OutputFormat::Native, None, Some(0.95))
            .await
            .unwrap();

        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].entity_type, "FR_NIR");
        assert!(result.text.unwrap().contains("john@example.com")); // below threshold, untouched
    }
}
