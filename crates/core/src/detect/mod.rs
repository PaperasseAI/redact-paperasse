pub mod tier_a;
#[cfg(feature = "tier-b")]
pub mod tier_b;

pub use tier_a::TierA;
#[cfg(feature = "tier-b")]
pub use tier_b::TierB;

use crate::types::{Entity, WordBox};

/// Attach pixel bounding boxes to entities that arrived without one, by
/// routing each span through the same `word_boxes` union lookup Tier A uses.
///
/// This is the piece that lets Tier B (Presidio NER) results participate in
/// image/PDF redaction at all: Tier B only ever sees extracted text, so its
/// entities come back `bbox: None`, and an entity without a box cannot be
/// drawn. The lookup is a pure function of the ingested document, so it
/// lives here rather than inside the REST client — testable without a
/// network, and usable by any future source of span-only detections.
///
/// An entity whose span no word box overlaps keeps `bbox: None`. Deciding
/// what that means (skip it, or refuse to produce a document that silently
/// misses it) is the caller's policy call — for pixel redaction the engine
/// fails closed, see `unplaceable_types`.
pub fn attach_bboxes(entities: &mut [Entity], word_boxes: &[WordBox]) {
    for entity in entities.iter_mut() {
        if entity.bbox.is_none() {
            entity.bbox = tier_a::union_bbox(word_boxes, entity.span.start, entity.span.end);
        }
    }
}

/// The entity types in `entities` that still have no bounding box — the
/// ones a pixel redaction pass would silently fail to draw. Returned as a
/// deduplicated list so an error message can name what would be missed,
/// because "this document could not be safely redacted, these types are
/// why" is actionable and "error" is not.
pub fn unplaceable_types(entities: &[Entity]) -> Vec<String> {
    let mut types: Vec<String> = entities
        .iter()
        .filter(|e| e.bbox.is_none())
        .map(|e| e.entity_type.clone())
        .collect();
    types.sort();
    types.dedup();
    types
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    use crate::types::{BoundingBox, DetectionSource, Span};

    fn word(start: usize, end: usize, x: f32, page: u32) -> WordBox {
        WordBox {
            span: Span { start, end },
            bbox: BoundingBox {
                page,
                x,
                y: 10.0,
                width: 20.0,
                height: 10.0,
            },
        }
    }

    fn span_entity(start: usize, end: usize) -> Entity {
        Entity {
            entity_type: "PERSON".into(),
            span: Span { start, end },
            score: 0.85,
            bbox: None,
            source: DetectionSource::TierB,
        }
    }

    #[test]
    fn a_multi_word_span_gets_the_union_of_its_words() {
        // A name is usually two OCR tokens; the box must cover both, not
        // just the one the span happens to start in.
        let words = [word(0, 5, 0.0, 1), word(6, 11, 30.0, 1)];
        let mut entities = [span_entity(0, 11)];
        attach_bboxes(&mut entities, &words);
        let bbox = entities[0].bbox.expect("span overlaps both words");
        assert_eq!(bbox.x, 0.0);
        assert_eq!(bbox.width, 50.0);
    }

    #[test]
    fn a_span_no_word_covers_stays_unplaced_and_is_reported() {
        let words = [word(0, 5, 0.0, 1)];
        let mut entities = [span_entity(40, 50)];
        attach_bboxes(&mut entities, &words);
        assert!(entities[0].bbox.is_none());
        assert_eq!(unplaceable_types(&entities), vec!["PERSON".to_string()]);
    }

    #[test]
    fn an_existing_bbox_is_never_overwritten() {
        // Tier A already placed its own boxes; routing must be additive.
        let words = [word(0, 5, 99.0, 2)];
        let mut entities = [span_entity(0, 5)];
        entities[0].bbox = Some(BoundingBox {
            page: 1,
            x: 1.0,
            y: 1.0,
            width: 1.0,
            height: 1.0,
        });
        attach_bboxes(&mut entities, &words);
        assert_eq!(entities[0].bbox.unwrap().x, 1.0);
    }

    #[test]
    fn placed_entities_are_not_reported_unplaceable() {
        let words = [word(0, 5, 0.0, 1)];
        let mut entities = [span_entity(0, 5)];
        attach_bboxes(&mut entities, &words);
        assert!(unplaceable_types(&entities).is_empty());
    }
}
