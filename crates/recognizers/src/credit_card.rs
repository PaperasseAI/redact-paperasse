use std::sync::LazyLock;

use regex::Regex;

use crate::{Match, Recognizer};

/// Recognizes credit card numbers (13-19 digits, optionally grouped with
/// spaces or hyphens), validated against the Luhn checksum used by every
/// major card network (Visa/Mastercard/Amex/Discover/etc.).
pub struct CreditCard;

static CARD_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:\d[ -]?){12,18}\d\b").expect("CREDIT_CARD pattern is a fixed, tested literal")
});

impl CreditCard {
    /// Luhn checksum: from the rightmost digit, double every second digit
    /// (subtracting 9 if the result exceeds 9), sum everything, and the
    /// total must be a multiple of 10.
    fn validate(candidate: &str) -> bool {
        let digits: Vec<u32> = candidate.chars().filter_map(|c| c.to_digit(10)).collect();
        if !(13..=19).contains(&digits.len()) {
            return false;
        }

        let sum: u32 = digits
            .iter()
            .rev()
            .enumerate()
            .map(|(i, &d)| {
                if i % 2 == 1 {
                    let doubled = d * 2;
                    if doubled > 9 {
                        doubled - 9
                    } else {
                        doubled
                    }
                } else {
                    d
                }
            })
            .sum();

        sum.is_multiple_of(10)
    }
}

impl Recognizer for CreditCard {
    fn entity_type(&self) -> &'static str {
        "CREDIT_CARD"
    }

    fn analyze(&self, text: &str) -> Vec<Match> {
        CARD_PATTERN
            .find_iter(text)
            .filter(|m| Self::validate(m.as_str()))
            .map(|m| Match {
                start: m.start(),
                end: m.end(),
                score: 1.0,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(text: &str) -> Vec<(usize, usize)> {
        CreditCard
            .analyze(text)
            .into_iter()
            .map(|m| (m.start, m.end))
            .collect()
    }

    // Standard payment-gateway test numbers — Luhn-valid by construction,
    // not real cards.

    #[test]
    fn valid_visa_compact() {
        assert_eq!(spans("4111111111111111"), vec![(0, 16)]);
    }

    #[test]
    fn valid_visa_spaced() {
        assert_eq!(spans("4111 1111 1111 1111"), vec![(0, 19)]);
    }

    #[test]
    fn valid_amex_dashed() {
        // Amex groups as 4-6-5, not 4-4-4-4.
        assert_eq!(spans("3782-822463-10005"), vec![(0, 17)]);
    }

    #[test]
    fn bad_checksum_rejected() {
        assert!(spans("4111111111111112").is_empty());
    }

    #[test]
    fn found_within_surrounding_text() {
        assert_eq!(
            spans("card on file: 4111111111111111 exp 12/29"),
            vec![(14, 30)]
        );
    }

    #[test]
    fn score_is_one_when_valid() {
        assert_eq!(CreditCard.analyze("4111111111111111")[0].score, 1.0);
    }
}
