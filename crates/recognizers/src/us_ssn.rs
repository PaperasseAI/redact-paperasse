use std::sync::LazyLock;

use regex::Regex;

use crate::{Match, Recognizer};

/// Recognizes US Social Security Numbers (NNN-NN-NNNN, dash- or
/// space-separated). SSNs have no checksum — `validate` instead encodes the
/// same structural exclusion rules Presidio's own `UsSsnRecognizer` uses:
/// the area number can't be 000/666/900-999, the group number can't be 00,
/// and the serial number can't be 0000 (all reserved/never-issued ranges).
///
/// Deliberately doesn't match a bare 9-digit run with no separator: that's
/// ambiguous with phone numbers, account numbers, and other 9-digit
/// identifiers, and Presidio's own pattern requires a separator too.
pub struct UsSsn;

static SSN_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{3}[ -]\d{2}[ -]\d{4}\b").expect("US_SSN pattern is a fixed, tested literal")
});

impl UsSsn {
    fn validate(candidate: &str) -> bool {
        let digits: Vec<u32> = candidate.chars().filter_map(|c| c.to_digit(10)).collect();
        if digits.len() != 9 {
            return false;
        }

        let area = digits[0] * 100 + digits[1] * 10 + digits[2];
        let group = digits[3] * 10 + digits[4];
        let serial = digits[5] * 1000 + digits[6] * 100 + digits[7] * 10 + digits[8];

        area != 0 && area != 666 && area < 900 && group != 0 && serial != 0
    }
}

impl Recognizer for UsSsn {
    fn entity_type(&self) -> &'static str {
        "US_SSN"
    }

    fn analyze(&self, text: &str) -> Vec<Match> {
        SSN_PATTERN
            .find_iter(text)
            .filter(|m| Self::validate(m.as_str()))
            .map(|m| Match {
                start: m.start(),
                end: m.end(),
                score: 0.85,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(text: &str) -> Vec<(usize, usize)> {
        UsSsn
            .analyze(text)
            .into_iter()
            .map(|m| (m.start, m.end))
            .collect()
    }

    #[test]
    fn dash_separated_valid() {
        assert_eq!(spans("536-90-4399"), vec![(0, 11)]);
    }

    #[test]
    fn space_separated_valid() {
        assert_eq!(spans("536 90 4399"), vec![(0, 11)]);
    }

    #[test]
    fn area_000_rejected() {
        assert!(spans("000-12-3456").is_empty());
    }

    #[test]
    fn area_666_rejected() {
        assert!(spans("666-12-3456").is_empty());
    }

    #[test]
    fn area_900_plus_rejected() {
        assert!(spans("901-12-3456").is_empty());
    }

    #[test]
    fn group_00_rejected() {
        assert!(spans("536-00-4399").is_empty());
    }

    #[test]
    fn serial_0000_rejected() {
        assert!(spans("536-90-0000").is_empty());
    }

    #[test]
    fn compact_form_not_matched() {
        assert!(spans("536904399").is_empty());
    }

    #[test]
    fn found_within_surrounding_text() {
        assert_eq!(
            spans("SSN on file: 536-90-4399 for verification"),
            vec![(13, 24)]
        );
    }
}
