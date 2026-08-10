use std::sync::LazyLock;

use regex::Regex;

use crate::{Match, Recognizer};

/// Recognizes IBANs (International Bank Account Numbers), validated against
/// the ISO 7064 mod-97 checksum every IBAN carries: move the first four
/// characters (country code + check digits) to the end, convert letters to
/// numbers (A=10..Z=35), and the resulting number mod 97 must equal 1.
pub struct Iban;

static IBAN_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Z]{2}\d{2}(?:[ ]?[A-Z0-9]{4})*[ ]?[A-Z0-9]{0,3}\b")
        .expect("IBAN_CODE pattern is a fixed, tested literal")
});

impl Iban {
    fn validate(candidate: &str) -> bool {
        let cleaned: String = candidate
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_uppercase)
            .collect();

        let len = cleaned.chars().count();
        if !(15..=34).contains(&len) {
            return false;
        }
        if !cleaned.chars().take(2).all(|c| c.is_ascii_alphabetic()) {
            return false;
        }
        if !cleaned.chars().skip(2).take(2).all(|c| c.is_ascii_digit()) {
            return false;
        }

        // Move the country code + check digits to the end, then convert
        // every letter to its two-digit number (A=10..Z=35).
        let rearranged = cleaned.chars().skip(4).chain(cleaned.chars().take(4));
        let mut numeric = String::with_capacity(len * 2);
        for c in rearranged {
            if c.is_ascii_digit() {
                numeric.push(c);
            } else if c.is_ascii_uppercase() {
                numeric.push_str(&(c as u32 - 'A' as u32 + 10).to_string());
            } else {
                return false;
            }
        }

        mod97(&numeric) == 1
    }
}

/// Iterative mod-97, processing one digit at a time so the (up to 34-digit)
/// numeric form never has to fit in a native integer type.
fn mod97(numeric: &str) -> u32 {
    let mut remainder: u64 = 0;
    for c in numeric.chars() {
        let digit = c.to_digit(10).expect("numeric string is digits only") as u64;
        remainder = (remainder * 10 + digit) % 97;
    }
    remainder as u32
}

impl Recognizer for Iban {
    fn entity_type(&self) -> &'static str {
        "IBAN_CODE"
    }

    fn analyze(&self, text: &str) -> Vec<Match> {
        IBAN_PATTERN
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
        Iban.analyze(text)
            .into_iter()
            .map(|m| (m.start, m.end))
            .collect()
    }

    // The standard example IBANs from ISO 13616 / Wikipedia's IBAN article —
    // both manually verified against the mod-97 algorithm above, not just
    // assumed valid.

    #[test]
    fn gb_compact_valid() {
        assert_eq!(spans("GB82WEST12345698765432"), vec![(0, 22)]);
    }

    #[test]
    fn gb_spaced_valid() {
        assert_eq!(spans("GB82 WEST 1234 5698 7654 32"), vec![(0, 27)]);
    }

    #[test]
    fn de_compact_valid() {
        assert_eq!(spans("DE89370400440532013000"), vec![(0, 22)]);
    }

    #[test]
    fn bad_checksum_rejected() {
        assert!(spans("GB82WEST12345698765433").is_empty());
    }

    #[test]
    fn too_short_rejected() {
        assert!(spans("GB82WEST").is_empty());
    }

    #[test]
    fn found_within_surrounding_text() {
        assert_eq!(
            spans("wire to GB82WEST12345698765432 by Friday"),
            vec![(8, 30)]
        );
    }

    #[test]
    fn score_is_one_when_valid() {
        assert_eq!(Iban.analyze("GB82WEST12345698765432")[0].score, 1.0);
    }
}
