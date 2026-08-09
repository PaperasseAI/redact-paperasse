//! `paperasse-privacy-core`: image/PDF/text in, redacted image/PDF/text/markdown
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
pub use types::{DetectionSource, Entity, ExtractedDocument, Input, OutputFormat, RedactionResult};

use detect::TierA;
use ingest::{DefaultIngestor, Ingestor};

/// The default pipeline: ingest → Tier A detect → redact. Tier B (Presidio)
/// is opt-in — construct `detect::TierB` separately, merge its results with
/// Tier A's, and call the `redact` module's functions directly when broader
/// NER coverage is worth the extra hop than this convenience wrapper
/// provides.
pub struct Engine {
    ingestor: Box<dyn Ingestor>,
    tier_a: TierA,
    // TODO: this is built separately from `DefaultIngestor`'s own internal
    // `LiteparseIngestor` config (both currently just `Default::default()`,
    // so no divergence risk yet) because `redact_pdf_bytes` needs the same
    // DPI the ingest pass used, and `Ingestor` is a trait object with no
    // way to expose that. Add `Engine::with_liteparse_config` once a caller
    // actually needs to customize it, threading one config through both.
    liteparse_config: liteparse::config::LiteParseConfig,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            ingestor: Box::new(DefaultIngestor::default()),
            tier_a: TierA::default(),
            liteparse_config: liteparse::config::LiteParseConfig::default(),
        }
    }
}

impl Engine {
    /// Build an engine with a specific ingestor — e.g.
    /// `Engine::new(Box::new(ingest::AnydocIngestor))` to force the
    /// lightweight, no-native-binary path when the caller already knows
    /// none of its documents are scans.
    pub fn new(ingestor: Box<dyn Ingestor>) -> Self {
        Self {
            ingestor,
            tier_a: TierA::default(),
            liteparse_config: liteparse::config::LiteParseConfig::default(),
        }
    }

    /// Run the default pipeline. `format` controls the output shape:
    /// `OutputFormat::Native` mirrors the input's own type (redacted image
    /// stays an image, redacted PDF stays a PDF); `OutputFormat::Markdown`
    /// forces structured markdown output regardless of input type.
    pub async fn process(&self, input: Input, format: OutputFormat) -> Result<RedactionResult, EngineError> {
        // Only a Native-format Pdf/Image needs pixel coordinates out of
        // ingestion — Text never does, and Markdown output never does
        // regardless of input type (see `Ingestor::ingest`'s doc comment).
        let needs_boxes = format == OutputFormat::Native && !matches!(input, Input::Text(_));
        let doc = self.ingestor.ingest(&input, needs_boxes).await?;
        let entities = self.tier_a.analyze(&doc);

        if format == OutputFormat::Markdown {
            return Ok(redact::redact_text(&doc, &entities, format));
        }

        match input {
            Input::Text(_) => Ok(redact::redact_text(&doc, &entities, format)),
            Input::Image { bytes, .. } => {
                let redacted_bytes = redact::redact_image_bytes(&bytes, &entities)?;
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

    #[tokio::test]
    async fn redacts_fr_nir_in_plain_text() {
        let engine = Engine::default();
        let result = engine
            .process(
                Input::Text("mon NIR est 291059933807692, merci.".to_string()),
                OutputFormat::Native,
            )
            .await
            .expect("text pipeline never hits the unimplemented pixel path");

        assert_eq!(result.entities.len(), 1);
        assert_eq!(result.entities[0].entity_type, "FR_NIR");
        assert!(!result.text.unwrap().contains("291059933807692"));
    }
}
