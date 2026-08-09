use std::sync::LazyLock;

use regex::Regex;

use crate::{Match, Recognizer};

/// Recognizes the French NIR (numéro de sécurité sociale / carte vitale number),
/// validated against INSEE's mod-97 checksum. Ported from the `FrNirRecognizer`
/// added to Presidio on the `paperasse-fr-nir` branch (see that PR for the
/// format reference and the OCR'd real-document test that motivated it).
///
/// Format: S YY MM DD CCC OOO KK — 13 significant digits followed by a 2-digit
/// checksum, written with or without spaces/dots (e.g. "2 91 05 99 338 076 92"
/// or "291059933807692"). Reference: <https://www.insee.fr/fr/metadonnees/definition/c1409>
pub struct FrNir;

static NIR_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b[12][ .]?\d{2}[ .]?(?:0[1-9]|1[0-2]|[2-9]\d)[ .]?(?:\d{2}|2[AB])[ .]?\d{3}[ .]?\d{3}[ .]?\d{2}\b",
    )
    .expect("FR_NIR pattern is a fixed, tested literal")
});

impl FrNir {
    /// INSEE mod-97 checksum: `97 - (first_13_digits % 97) == last_2_digits`,
    /// with Corsican department codes (2A/2B) substituted to 19/18 first.
    fn validate(candidate: &str) -> bool {
        let cleaned: String = candidate
            .chars()
            .filter(|c| !matches!(c, ' ' | '.' | '-'))
            .flat_map(char::to_uppercase)
            .collect();
        if cleaned.chars().count() != 15 {
            return false;
        }

        let digits_part: String = cleaned.chars().take(13).collect();
        let key_part: String = cleaned.chars().skip(13).collect();

        let checksum_input = digits_part.replace("2A", "19").replace("2B", "18");
        let Ok(n) = checksum_input.parse::<u64>() else {
            return false;
        };
        let Ok(key) = key_part.parse::<u64>() else {
            return false;
        };

        97 - (n % 97) == key
    }
}

impl Recognizer for FrNir {
    fn entity_type(&self) -> &'static str {
        "FR_NIR"
    }

    fn analyze(&self, text: &str) -> Vec<Match> {
        NIR_PATTERN
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
        FrNir
            .analyze(text)
            .into_iter()
            .map(|m| (m.start, m.end))
            .collect()
    }

    // Same fixtures as presidio-analyzer/tests/test_fr_nir_recognizer.py on the
    // paperasse-fr-nir branch — keep these two in sync if either changes.

    #[test]
    fn compact_valid() {
        assert_eq!(spans("291059933807692"), vec![(0, 15)]);
    }

    #[test]
    fn spaced_valid() {
        assert_eq!(spans("1 85 05 78 120 123 27"), vec![(0, 21)]);
    }

    #[test]
    fn dot_valid() {
        assert_eq!(spans("2.96.01.12.345.678.59"), vec![(0, 21)]);
    }

    #[test]
    fn corsican_department_valid() {
        assert_eq!(spans("185032A03411208"), vec![(0, 15)]);
    }

    #[test]
    fn bad_checksum_rejected() {
        assert!(spans("291059933807693").is_empty());
    }

    #[test]
    fn bad_month_rejected_by_regex() {
        assert!(spans("291139933807692").is_empty());
    }

    #[test]
    fn wrong_length_rejected() {
        assert!(spans("29105993380769").is_empty());
        assert!(spans("2910599338076920").is_empty());
    }

    #[test]
    fn hyphen_separated_not_supported() {
        assert!(spans("2-91-05-99-338-076-92").is_empty());
    }

    #[test]
    fn found_within_surrounding_text() {
        // Byte offsets, not char offsets: "numéro"/"sécurité" contribute
        // three 2-byte UTF-8 characters before the match, so this starts
        // three bytes later than a naive char-count would suggest — the
        // same distinction that motivated fixing `redact::mask_text` to
        // work on byte spans directly instead of a `Vec<char>`.
        assert_eq!(
            spans("mon numéro de sécurité sociale est 2 91 05 99 338 076 92"),
            vec![(38, 59)]
        );
    }

    #[test]
    fn score_is_one_when_valid() {
        let m = &FrNir.analyze("291059933807692")[0];
        assert_eq!(m.score, 1.0);
    }
}
