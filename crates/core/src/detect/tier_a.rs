use paperasse_privacy_recognizers::{Recognizer, default_registry};

use crate::types::{DetectionSource, Entity, ExtractedDocument, Span};

/// The default, in-process detection pass: regex+checksum recognizers only
/// (see `paperasse-privacy-recognizers`), zero network dependency. This is
/// the whole reason to ship as a native binding instead of only a REST API —
/// most PII an agent needs to worry about (identifiers with a fixed format:
/// SSNs, NIRs, IBANs, emails) doesn't need general NER to catch reliably,
/// and NER is where Presidio's own accuracy gets noisy (see the FR_NIR work
/// on `paperasse-fr-nir`: the checksum-validated match was the reliable
/// signal, the NER layer's guesses were the wrong ones).
pub struct TierA {
    recognizers: Vec<Box<dyn Recognizer>>,
}

impl Default for TierA {
    fn default() -> Self {
        Self {
            recognizers: default_registry(),
        }
    }
}

impl TierA {
    pub fn analyze(&self, doc: &ExtractedDocument) -> Vec<Entity> {
        let mut entities = Vec::new();
        for recognizer in &self.recognizers {
            for m in recognizer.analyze(&doc.text) {
                let bbox = doc
                    .word_boxes
                    .iter()
                    .find(|wb| wb.span.start <= m.start && m.end <= wb.span.end)
                    .map(|wb| wb.bbox);
                entities.push(Entity {
                    entity_type: recognizer.entity_type().to_string(),
                    span: Span {
                        start: m.start,
                        end: m.end,
                    },
                    score: m.score,
                    bbox,
                    source: DetectionSource::TierA,
                });
            }
        }
        entities
    }
}
