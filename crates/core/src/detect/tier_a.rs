use paperasse_privacy_recognizers::{default_registry, Recognizer};

use crate::types::{BoundingBox, DetectionSource, Entity, ExtractedDocument, Span};

/// The bounding box to redact for a match spanning `[start, end)`: the
/// union of every `word_box` it overlaps, not just one it's fully
/// contained in.
///
/// A real bug found while testing this crate's Node binding against a
/// synthetic fixture: a spaced NIR ("2 91 05 99 338 076 92") OCRs as seven
/// separate word tokens, so the regex match spans all seven — but the
/// original lookup (`find` on a single `word_box` fully containing the
/// match) never matches a multi-token span, silently returning `bbox:
/// None` and skipping the redaction box entirely. A real photographed
/// document's *compact* NIR ("291059933807692", no spaces) OCRs as one
/// token, which is why that case worked and this one didn't — the bug was
/// there the whole time, just not exercised by the one format tested.
///
/// Boxes are unioned only within the first page found among the
/// overlapping set — a match spanning a page boundary (rare; text is
/// joined continuously across pages) doesn't get a nonsensical
/// cross-page rectangle.
fn union_bbox(
    word_boxes: &[crate::types::WordBox],
    start: usize,
    end: usize,
) -> Option<BoundingBox> {
    let mut overlapping = word_boxes
        .iter()
        .filter(|wb| wb.span.start < end && start < wb.span.end)
        .map(|wb| wb.bbox);

    let first = overlapping.next()?;
    let (page, mut x0, mut y0, mut x1, mut y1) = (
        first.page,
        first.x,
        first.y,
        first.x + first.width,
        first.y + first.height,
    );

    for b in overlapping.filter(|b| b.page == page) {
        x0 = x0.min(b.x);
        y0 = y0.min(b.y);
        x1 = x1.max(b.x + b.width);
        y1 = y1.max(b.y + b.height);
    }

    Some(BoundingBox {
        page,
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

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
                let bbox = union_bbox(&doc.word_boxes, m.start, m.end);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WordBox;

    fn word(start: usize, end: usize, page: u32, x: f32) -> WordBox {
        WordBox {
            span: Span { start, end },
            bbox: BoundingBox {
                page,
                x,
                y: 100.0,
                width: 10.0,
                height: 12.0,
            },
        }
    }

    #[test]
    fn single_token_match_uses_its_own_box() {
        // The case that already worked: a match fully inside one word_box.
        let boxes = vec![word(0, 5, 1, 20.0)];
        let bbox = union_bbox(&boxes, 0, 5).unwrap();
        assert_eq!(bbox.x, 20.0);
        assert_eq!(bbox.width, 10.0);
    }

    #[test]
    fn multi_token_match_unions_every_overlapping_box() {
        // The bug: "2 91 05 99 338 076 92" OCRs as 7 tokens, the regex
        // match spans all of them. Simulate 3 adjacent word_boxes and a
        // match spanning all three — the result must cover all three, not
        // come back None.
        let boxes = vec![
            word(0, 1, 1, 0.0),  // "2"      at x=0..10
            word(2, 4, 1, 15.0), // "91"     at x=15..25
            word(5, 7, 1, 30.0), // "05"     at x=30..40
        ];
        let bbox = union_bbox(&boxes, 0, 7).unwrap();
        assert_eq!(bbox.page, 1);
        assert_eq!(bbox.x, 0.0);
        assert_eq!(bbox.x + bbox.width, 40.0); // covers the rightmost token's right edge
    }

    #[test]
    fn no_overlapping_box_returns_none() {
        let boxes = vec![word(100, 105, 1, 0.0)];
        assert!(union_bbox(&boxes, 0, 5).is_none());
    }

    #[test]
    fn ignores_boxes_on_a_different_page_than_the_first_match() {
        let boxes = vec![word(0, 3, 1, 0.0), word(3, 6, 2, 999.0)];
        let bbox = union_bbox(&boxes, 0, 6).unwrap();
        assert_eq!(bbox.page, 1);
        assert_eq!(bbox.x, 0.0); // the page-2 box's x=999 must not leak in
    }
}
