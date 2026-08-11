use std::sync::LazyLock;

use regex::Regex;

use crate::{Match, Recognizer};

/// Recognizes phone numbers in French, US, and `+`-international formats.
/// Deliberately narrower than Presidio's own `PhoneRecognizer` (which uses
/// Google's `phonenumbers` library for real region-aware validation) — this
/// is a shape + digit-count sanity check, not true validation, since phone
/// numbers have no checksum.
///
/// The French branch exists because for a long time it didn't: the pattern
/// knew US 3-3-4 groupings and `+`-international, and its one French test
/// was a `+33` case — so `01 56 35 91 80`, the way every French person
/// writes their number, sailed through a real tax form unredacted. The
/// separators between pairs are individually optional, because OCR merges
/// them unpredictably: the form that exposed this read back as
/// `0156 35 9180`, which is the same ten digits in groups no human would
/// type.
///
/// Every match must still carry at least one non-digit character (`+`,
/// parens, or a separator): a bare 7-15 digit run is too ambiguous with
/// other identifiers (account numbers, order references, SSNs) to claim as
/// a phone number by default — same reasoning as `UsSsn` refusing a compact
/// 9-digit run. This means a fully compact `0156359180` is deliberately NOT
/// matched. If OCR drops every separator we miss it; that trade is
/// documented here so changing it is a decision, not an accident.
pub struct PhoneNumber;

static PHONE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\+\d{1,3}[ .-]?\d{1,4}(?:[ .-]?\d{1,4}){1,4}|\(\d{3}\)[ .-]?\d{3}[ .-]?\d{4}|\b0[1-9](?:[ .-]?\d\d){4}\b|\b\d{3}[ .-]\d{3}[ .-]\d{4}\b",
    )
    .expect("PHONE_NUMBER pattern is a fixed, tested literal")
});

impl PhoneNumber {
    /// E.164 allows up to 15 digits total; 7 is a reasonable floor for the
    /// shortest real local numbers this pattern can match. The non-digit
    /// requirement enforces the "no bare runs" policy for the French
    /// branch, whose per-pair separators are individually optional in the
    /// regex (the regex crate has no lookahead to demand one there).
    fn validate(candidate: &str) -> bool {
        let digit_count = candidate.chars().filter(|c| c.is_ascii_digit()).count();
        (7..=15).contains(&digit_count) && candidate.chars().any(|c| !c.is_ascii_digit())
    }
}

impl Recognizer for PhoneNumber {
    fn entity_type(&self) -> &'static str {
        "PHONE_NUMBER"
    }

    fn analyze(&self, text: &str) -> Vec<Match> {
        PHONE_PATTERN
            .find_iter(text)
            .filter(|m| Self::validate(m.as_str()))
            .map(|m| Match {
                start: m.start(),
                end: m.end(),
                score: 0.75,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(text: &str) -> Vec<(usize, usize)> {
        PhoneNumber
            .analyze(text)
            .into_iter()
            .map(|m| (m.start, m.end))
            .collect()
    }

    #[test]
    fn us_dashed_valid() {
        assert_eq!(spans("212-555-0143"), vec![(0, 12)]);
    }

    #[test]
    fn us_parens_valid() {
        assert_eq!(spans("(212) 555-0143"), vec![(0, 14)]);
    }

    #[test]
    fn international_valid() {
        assert_eq!(spans("+33 6 12 34 56 78"), vec![(0, 17)]);
    }

    #[test]
    fn french_pairs_spaced() {
        // The national format: ten digits in five pairs. This is how every
        // French person writes their number, and it went unmatched until a
        // real form exposed it.
        assert_eq!(spans("01 56 35 91 80"), vec![(0, 14)]);
        assert_eq!(spans("06 12 34 56 78"), vec![(0, 14)]);
    }

    #[test]
    fn french_pairs_dotted() {
        assert_eq!(spans("01.56.35.91.80"), vec![(0, 14)]);
    }

    #[test]
    fn french_ocr_irregular_grouping() {
        // Verbatim from the CFE form that exposed the bug: OCR merged the
        // pair separators into groups no human would type. The per-pair
        // separators are optional precisely for this.
        assert_eq!(spans("téléphonez au : 0156 35 9180"), vec![(18, 30)]);
    }

    #[test]
    fn french_compact_refused_by_policy() {
        // All ten digits, no separator: deliberately not matched. A bare
        // digit run is too ambiguous with account and order numbers; see
        // the module doc before changing this.
        assert!(spans("0156359180").is_empty());
    }

    #[test]
    fn a_siret_is_not_a_phone_number() {
        // 14 digits in SIRET grouping. The trailing establishment number
        // must not be claimed by any branch.
        assert!(spans("944 681 634 00019").is_empty());
    }

    #[test]
    fn no_separator_not_matched() {
        assert!(spans("2125550143").is_empty());
    }

    #[test]
    fn found_within_surrounding_text() {
        assert_eq!(spans("Call me at 212-555-0143 today"), vec![(11, 23)]);
    }
}
