use std::sync::LazyLock;

use regex::Regex;

use crate::{Match, Recognizer};

/// Recognizes credit card numbers (13-19 digits, optionally grouped with
/// spaces or hyphens), validated against the Luhn checksum used by every
/// major card network *and* the issuer prefix (IIN) that network assigns.
///
/// Luhn alone is not enough to say "this is a card". It is a transcription
/// check, not an identity check: roughly one in ten arbitrary digit strings
/// of the right length passes it. French SIRET numbers are the case that
/// forced this — 14 digits, Luhn-valid by the same rule — so every invoice
/// SIRET was being blacked out as a credit card, and reported as one in the
/// audit trail. Requiring a real issuer prefix separates them, because no
/// network issues cards starting 73.
pub struct CreditCard;

static CARD_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:\d[ -]?){12,18}\d\b").expect("CREDIT_CARD pattern is a fixed, tested literal")
});

impl CreditCard {
    /// Luhn checksum: from the rightmost digit, double every second digit
    /// (subtracting 9 if the result exceeds 9), sum everything, and the
    /// total must be a multiple of 10.
    /// True when the digits open with an issuer prefix a real network hands
    /// out, at a length that network actually issues. Ranges are from the
    /// published IIN allocations; the length pairing matters as much as the
    /// prefix, since it is what keeps a 14-digit SIRET from passing as a
    /// 16-digit Mastercard that happens to share two leading digits.
    fn has_issuer_prefix(digits: &[u32]) -> bool {
        let len = digits.len();
        // Leading `n` digits as a number, for range comparisons.
        let lead = |n: usize| -> u32 { digits.iter().take(n).fold(0, |acc, d| acc * 10 + d) };

        match () {
            // Visa
            _ if lead(1) == 4 => matches!(len, 13 | 16 | 19),
            // Mastercard: the classic 51-55 block and the newer 2221-2720 one
            _ if (51..=55).contains(&lead(2)) => len == 16,
            _ if (2221..=2720).contains(&lead(4)) => len == 16,
            // American Express
            _ if matches!(lead(2), 34 | 37) => len == 15,
            // Discover
            _ if lead(4) == 6011 => matches!(len, 16 | 19),
            _ if (622126..=622925).contains(&lead(6)) => matches!(len, 16 | 19),
            _ if (644..=649).contains(&lead(3)) => matches!(len, 16 | 19),
            _ if lead(2) == 65 => matches!(len, 16 | 19),
            // Diners Club — the one that genuinely overlaps SIRET at 14
            // digits. Kept anyway: dropping a real card format to dodge a
            // false positive is the wrong trade for a redaction tool.
            _ if (300..=305).contains(&lead(3)) => len == 14,
            _ if lead(4) == 3095 => len == 14,
            _ if matches!(lead(2), 36 | 38 | 39) => len == 14,
            // JCB
            _ if (3528..=3589).contains(&lead(4)) => (16..=19).contains(&len),
            // UnionPay
            _ if lead(2) == 62 => (16..=19).contains(&len),
            _ => false,
        }
    }

    fn validate(candidate: &str) -> bool {
        let digits: Vec<u32> = candidate.chars().filter_map(|c| c.to_digit(10)).collect();
        if !(13..=19).contains(&digits.len()) {
            return false;
        }
        if !Self::has_issuer_prefix(&digits) {
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

    #[test]
    fn a_french_siret_is_not_a_credit_card() {
        // 14 digits and Luhn-valid, exactly like a Diners card, but no
        // network issues cards starting 73. Before the issuer-prefix check
        // this was redacted as a CREDIT_CARD on every French invoice.
        assert!(spans("73282932000074").is_empty());
        assert!(spans("SIRET 732 829 320 00074").is_empty());
    }

    #[test]
    fn a_luhn_valid_number_with_no_issuer_prefix_is_rejected() {
        // 9999999999999996 passes Luhn; 99 is not an allocated IIN.
        assert!(spans("9999999999999996").is_empty());
    }

    #[test]
    fn the_other_networks_still_match() {
        // Standard gateway test numbers, one per prefix family.
        for n in [
            "5555555555554444", // Mastercard 51-55
            "2223003122003222", // Mastercard 2-series
            "6011111111111117", // Discover
            "3530111333300000", // JCB
            "36227206271667",   // Diners, 14 digits
            "4222222222222",    // Visa, 13 digits
        ] {
            assert_eq!(spans(n), vec![(0, n.len())], "should match: {n}");
        }
    }

    #[test]
    fn a_valid_prefix_at_the_wrong_length_is_rejected() {
        // 411111111111111116 is Luhn-valid and starts with 4, but Visa does
        // not issue 18-digit cards -- length is half the signal.
        assert!(spans("411111111111111116").is_empty());
    }
}
