use paperasse_privacy_recognizers::{default_registry, Recognizer};

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
    /// `entities` mirrors Presidio's `analyzer_entities`/`entities` filter:
    /// `None` runs every registered recognizer (the default); `Some(list)`
    /// runs only the recognizers whose `entity_type()` appears in `list`
    /// (e.g. `["FR_NIR"]` to redact only the NIR and leave emails/etc.
    /// untouched — see this session's Presidio testing for why that
    /// matters: a policy like "identifiers are fine, financial secrets
    /// aren't" needs per-entity-type selection, not all-or-nothing).
    /// Unrecognized names in the filter are silently ignored, same as
    /// Presidio does for an unknown entity type.
    ///
    /// `score_threshold` mirrors Presidio's own `score_threshold`: a match
    /// scoring below it is dropped. Most Tier A recognizers today report a
    /// fixed score regardless of context (checksum-validated ones like
    /// `FrNir` report 1.0 or don't match at all; `Email` always reports
    /// 0.9), so this mainly matters once a recognizer with real confidence
    /// variance exists, or when merging in Tier B's NER scores — but the
    /// filter is correct and available now rather than bolted on later.
    pub fn analyze(
        &self,
        doc: &ExtractedDocument,
        entities: Option<&[String]>,
        score_threshold: Option<f32>,
    ) -> Vec<Entity> {
        let mut out = Vec::new();
        for recognizer in &self.recognizers {
            if let Some(wanted) = entities {
                if !wanted.iter().any(|e| e == recognizer.entity_type()) {
                    continue;
                }
            }
            for m in recognizer.analyze(&doc.text) {
                if let Some(threshold) = score_threshold {
                    if m.score < threshold {
                        continue;
                    }
                }
                let bbox = doc
                    .word_boxes
                    .iter()
                    .find(|wb| wb.span.start <= m.start && m.end <= wb.span.end)
                    .map(|wb| wb.bbox);
                out.push(Entity {
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
        out
    }
}
