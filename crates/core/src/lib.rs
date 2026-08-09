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
//! See the crate-level `redact::redact_image` doc comment for the one piece
//! of this pipeline that's still a stub — everything else is wired end to
//! end.

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
/// Tier A's, and call `redact::redact_text`/`redact_image` directly when
/// broader NER coverage is worth the extra hop than this convenience
/// wrapper provides.
pub struct Engine {
    ingestor: Box<dyn Ingestor>,
    tier_a: TierA,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            ingestor: Box::new(DefaultIngestor::default()),
            tier_a: TierA::default(),
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
        }
    }

    /// Run the default pipeline. `format` controls the output shape:
    /// `OutputFormat::Native` mirrors the input's own type (redacted image
    /// stays an image, etc.); `OutputFormat::Markdown` forces structured
    /// markdown output regardless of input type.
    pub async fn process(&self, input: Input, format: OutputFormat) -> Result<RedactionResult, EngineError> {
        let doc = self.ingestor.ingest(&input).await?;
        let entities = self.tier_a.analyze(&doc);

        if format == OutputFormat::Markdown {
            return Ok(redact::redact_text(&doc, &entities, format));
        }

        match input {
            Input::Text(_) => Ok(redact::redact_text(&doc, &entities, format)),
            Input::Pdf(bytes) | Input::Image { bytes, .. } => {
                let redacted_bytes = redact::redact_image(&bytes, &entities)?;
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
