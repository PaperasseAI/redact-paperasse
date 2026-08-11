use std::sync::LazyLock;

use regex::Regex;

use crate::{Match, Recognizer};

/// Recognizes EU VAT identification numbers (`numéro de TVA
/// intracommunautaire`) for all 27 member states.
///
/// The two-letter country code is a strong anchor, which is what makes this
/// safe to run by default where a bare national number would not be: `DE`
/// followed by nine digits is a deliberate structure, not a coincidence.
///
/// Two confidence levels are reported, because the member states do not
/// offer the same guarantees:
///
/// * `1.0` — the country's check digits were verified (FR, BE, NL, LU).
/// * `0.8` — the country code and body shape are right, but the number was
///   not checksummed. Either the algorithm is not stable across the
///   country's number ranges, or the format admits letters where the check
///   is undefined. Claiming 1.0 here would be asserting a check that never
///   ran.
///
/// Note for anyone redacting French documents: `FR40303265045` *contains*
/// the SIREN `303265045`. Redacting a SIREN while leaving the VAT number
/// intact republishes it on the next line.
pub struct EuVat;

/// Country code, then the body, allowing one space or a non-breaking space
/// after the code — how it is usually typeset on an invoice.
static VAT_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(AT|BE|BG|CY|CZ|DE|DK|EE|EL|ES|FI|FR|HR|HU|IE|IT|LT|LU|LV|MT|NL|PL|PT|RO|SE|SI|SK)[ \u{00a0}]?([A-Z0-9]{2,13})\b",
    )
    .expect("EU_VAT pattern is a fixed, tested literal")
});

/// Digits of `s` as a number; `None` if any character is not a digit or the
/// value would not fit.
fn digits_as_u64(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u64>().ok()
}

fn all_digits(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit())
}

fn digits_in(s: &str, range: std::ops::RangeInclusive<usize>) -> bool {
    range.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

impl EuVat {
    /// `Some(score)` when `body` is a well-formed VAT body for `country`.
    ///
    /// Returns 1.0 only where the check digits were actually verified, so a
    /// consumer filtering on `--score-threshold 1.0` gets exactly the
    /// checksummed subset rather than a mix of proven and merely plausible.
    fn validate(country: &str, body: &str) -> Option<f32> {
        match country {
            // ---- checksummed ----
            "FR" => {
                // 2-character key + 9-digit SIREN. Old-style numeric keys
                // satisfy key == (12 + 3 * (SIREN mod 97)) mod 97; newer
                // ones contain letters and carry no verifiable check, so
                // those fall back to format-only rather than being dropped.
                if body.len() != 11 {
                    return None;
                }
                let (key, siren) = body.split_at(2);
                if !all_digits(siren, 9) || !key.bytes().all(|b| b.is_ascii_alphanumeric()) {
                    return None;
                }
                match (digits_as_u64(key), digits_as_u64(siren)) {
                    (Some(k), Some(s)) => {
                        if (12 + 3 * (s % 97)) % 97 == k {
                            Some(1.0)
                        } else {
                            None
                        }
                    }
                    // Letter in the key: shape is right, check is undefined.
                    _ => Some(0.8),
                }
            }
            "BE" => {
                // 10 digits; the last two are 97 minus the first eight mod 97.
                let n = digits_as_u64(body).filter(|_| body.len() == 10)?;
                if (97 - (n / 100) % 97) == n % 100 {
                    Some(1.0)
                } else {
                    None
                }
            }
            "NL" => {
                // 9 digits, then 'B', then 2 digits. The 9 digits carry a
                // weighted mod-11 check.
                if body.len() != 12 || !body[9..10].eq_ignore_ascii_case("B") {
                    return None;
                }
                let head = &body[..9];
                if !all_digits(head, 9) || !all_digits(&body[10..], 2) {
                    return None;
                }
                let d: Vec<u64> = head.bytes().map(|b| u64::from(b - b'0')).collect();
                let sum: u64 = d[..8].iter().zip((2..=9).rev()).map(|(v, w)| v * w).sum();
                if sum % 11 == d[8] {
                    Some(1.0)
                } else {
                    None
                }
            }
            "LU" => {
                // 8 digits; first six mod 89 give the last two.
                let n = digits_as_u64(body).filter(|_| body.len() == 8)?;
                if (n / 100) % 89 == n % 100 {
                    Some(1.0)
                } else {
                    None
                }
            }

            // ---- format-checked only (0.8) ----
            "AT" => (body.len() == 9
                && body[..1].eq_ignore_ascii_case("U")
                && all_digits(&body[1..], 8))
            .then_some(0.8),
            "DE" | "EE" | "EL" | "PT" => all_digits(body, 9).then_some(0.8),
            "DK" | "FI" | "HU" | "MT" | "SI" => all_digits(body, 8).then_some(0.8),
            "BG" => digits_in(body, 9..=10).then_some(0.8),
            "CZ" => digits_in(body, 8..=10).then_some(0.8),
            "PL" | "SK" => all_digits(body, 10).then_some(0.8),
            "HR" | "IT" | "LV" => all_digits(body, 11).then_some(0.8),
            "SE" => all_digits(body, 12).then_some(0.8),
            "LT" => (all_digits(body, 9) || all_digits(body, 12)).then_some(0.8),
            "RO" => digits_in(body, 2..=10).then_some(0.8),
            "CY" => (body.len() == 9
                && all_digits(&body[..8], 8)
                && body.as_bytes()[8].is_ascii_alphabetic())
            .then_some(0.8),
            "ES" => (body.len() == 9
                && body.as_bytes()[0].is_ascii_alphanumeric()
                && all_digits(&body[1..8], 7)
                && body.as_bytes()[8].is_ascii_alphanumeric())
            .then_some(0.8),
            "IE" => {
                // Two live shapes: 7 digits + 1-2 letters, and the older
                // digit-letter-5digits-letter form.
                let b = body.as_bytes();
                let modern = (8..=9).contains(&body.len())
                    && all_digits(&body[..7], 7)
                    && b[7..].iter().all(|c| c.is_ascii_alphabetic());
                let legacy = body.len() == 8
                    && b[0].is_ascii_digit()
                    && b[1].is_ascii_alphanumeric()
                    && all_digits(&body[2..7], 5)
                    && b[7].is_ascii_alphabetic();
                (modern || legacy).then_some(0.8)
            }
            _ => None,
        }
    }
}

impl Recognizer for EuVat {
    fn entity_type(&self) -> &'static str {
        "EU_VAT"
    }

    fn analyze(&self, text: &str) -> Vec<Match> {
        VAT_PATTERN
            .captures_iter(text)
            .filter_map(|c| {
                let whole = c.get(0)?;
                let country = c.get(1)?.as_str().to_ascii_uppercase();
                let body = c.get(2)?.as_str().to_ascii_uppercase();
                let score = Self::validate(&country, &body)?;
                Some(Match {
                    start: whole.start(),
                    end: whole.end(),
                    score,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<(usize, usize, f32)> {
        EuVat
            .analyze(text)
            .into_iter()
            .map(|m| (m.start, m.end, m.score))
            .collect()
    }

    #[test]
    fn french_vat_with_a_valid_key_is_checksummed() {
        // FR40303265045 -- key 40 over SIREN 303265045.
        assert_eq!(found("FR40303265045"), vec![(0, 13, 1.0)]);
    }

    #[test]
    fn french_vat_with_a_wrong_key_is_rejected() {
        assert!(found("FR41303265045").is_empty());
    }

    #[test]
    fn french_vat_carries_the_siren_inside_it() {
        // The reason redacting SIREN alone is not enough: the same nine
        // digits sit in plain sight inside the VAT number.
        let m = found("TVA FR40303265045");
        assert_eq!(m.len(), 1);
        assert!("FR40303265045".contains("303265045"));
    }

    #[test]
    fn checksummed_countries_report_full_confidence() {
        for n in ["BE0776091951", "NL123456782B90", "LU12345613"] {
            let m = found(n);
            assert_eq!(m.len(), 1, "should match: {n}");
            assert_eq!(m[0].2, 1.0, "should be checksummed: {n}");
        }
    }

    #[test]
    fn a_broken_check_digit_is_rejected_not_downgraded() {
        // Shape is right in every case; only the check digits are wrong.
        // These must disappear, not reappear at 0.8 -- a failed checksum is
        // stronger evidence than an unrun one.
        for n in ["BE0776091952", "NL123456783B90", "LU12345614"] {
            assert!(found(n).is_empty(), "should be rejected: {n}");
        }
    }

    #[test]
    fn format_only_countries_report_reduced_confidence() {
        for n in ["DE123456789", "IT12345678901", "ATU12345678", "IE1234567FA"] {
            let m = found(n);
            assert_eq!(m.len(), 1, "should match: {n}");
            assert_eq!(m[0].2, 0.8, "should be format-only: {n}");
        }
    }

    #[test]
    fn a_space_after_the_country_code_is_allowed() {
        assert_eq!(found("FR 40303265045"), vec![(0, 14, 1.0)]);
    }

    #[test]
    fn lowercase_is_accepted() {
        assert_eq!(found("fr40303265045"), vec![(0, 13, 1.0)]);
    }

    #[test]
    fn a_non_eu_country_code_is_ignored() {
        // US and CH are not member states; GB left the union.
        for n in ["US123456789", "CH123456789", "GB123456789"] {
            assert!(found(n).is_empty(), "should not match: {n}");
        }
    }

    #[test]
    fn wrong_body_length_is_rejected() {
        assert!(found("DE12345678").is_empty());
        assert!(found("DE1234567890").is_empty());
    }
}
