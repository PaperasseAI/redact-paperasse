use std::sync::LazyLock;

use regex::Regex;

use crate::{Match, Recognizer};

/// Recognizes phone numbers in common US and international formats.
/// Deliberately narrower than Presidio's own `PhoneRecognizer` (which uses
/// Google's `phonenumbers` library for real region-aware validation) — this
/// is a shape + digit-count sanity check, not true validation, since phone
/// numbers have no checksum. Requires an explicit separator, parentheses, or
/// a leading `+`: a bare 7-15 digit run is too ambiguous with other
/// identifiers (account numbers, SSNs, etc.) to treat as a phone number by
/// default — same reasoning as `UsSsn` refusing to match a compact 9-digit
/// run.
pub struct PhoneNumber;

static PHONE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\+\d{1,3}[ .-]?\d{1,4}(?:[ .-]?\d{1,4}){1,4}|\(\d{3}\)[ .-]?\d{3}[ .-]?\d{4}|\b\d{3}[ .-]\d{3}[ .-]\d{4}\b",
    )
    .expect("PHONE_NUMBER pattern is a fixed, tested literal")
});

impl PhoneNumber {
    /// E.164 allows up to 15 digits total; 7 is a reasonable floor for the
    /// shortest real local numbers this pattern can match.
    fn validate(candidate: &str) -> bool {
        let digit_count = candidate.chars().filter(|c| c.is_ascii_digit()).count();
        (7..=15).contains(&digit_count)
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
    fn no_separator_not_matched() {
        assert!(spans("2125550143").is_empty());
    }

    #[test]
    fn found_within_surrounding_text() {
        assert_eq!(spans("Call me at 212-555-0143 today"), vec![(11, 23)]);
    }
}
