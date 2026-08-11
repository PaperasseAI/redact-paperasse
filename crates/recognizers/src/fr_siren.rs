use std::sync::LazyLock;

use regex::Regex;

use crate::{Match, Recognizer};

/// SIREN (9 digits) and SIRET (14 digits) — French company identifiers.
///
/// Both are Luhn-validated, and both need an anchor beyond the checksum:
/// Luhn passes roughly one arbitrary string in ten, so a bare digit rule
/// would claim invoice totals, order references and customer numbers on
/// every document it saw.
///
/// The anchors differ, and the asymmetry is deliberate:
///
/// * **SIRET**: a nearby label (`SIRET`, `SIREN`, `RCS`) *or* the canonical
///   3-3-3-5 grouping (`944 681 634 00019`). The grouping alone is a strong
///   claim — French formats no other quantity that way (thousands grouping
///   on 14 digits would be 2-3-3-3-3). This matters on forms: a real CFE
///   declaration carried its SIRET in an address block with the nearest
///   textual label 2,758 characters away on a later page, so any
///   label-window rule fails there. Measured, not estimated.
/// * **SIREN**: a nearby label, always. The tempting grouped-3-3-3 anchor
///   collides with French number typesetting itself: `123 456 789` is
///   exactly how millions are written in amounts, and one in ten of those
///   passes Luhn. A grouped-but-unlabelled SIREN rule would redact money.
///
/// They are registered, but the demo leaves them unticked, and that
/// asymmetry is deliberate. A SIREN is public data — the whole Sirene
/// database is open — and French invoices are legally required to carry it,
/// so blacking one out can invalidate the document for the purpose it exists
/// to serve. For an *entreprise individuelle* it is genuinely personal data,
/// tied in Sirene to what is often a home address. That is a real case, but
/// it is the minority one, so it should be chosen rather than assumed.
const LOOKBEHIND: usize = 48;

static SIREN_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{3}[ .]?\d{3}[ .]?\d{3}\b").expect("fixed literal"));

static SIRET_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{3}[ .]?\d{3}[ .]?\d{3}[ .]?\d{5}\b").expect("fixed literal")
});

/// La Poste is the documented exception, but narrower than it is usually
/// described: the SIREN 356000000 does satisfy Luhn (a test asserting
/// otherwise failed, which is how this got checked rather than assumed). It
/// is the *establishments* that do not — La Poste SIRETs validate by
/// digit-sum divisible by 5 instead. Excluding France's largest employer to
/// keep the code tidy is not a trade worth making.
const LA_POSTE_SIREN: &str = "356000000";

fn digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn luhn_ok(d: &str) -> bool {
    let sum: u32 = d
        .chars()
        .rev()
        .enumerate()
        .filter_map(|(i, c)| {
            let v = c.to_digit(10)?;
            Some(if i % 2 == 1 {
                if v * 2 > 9 {
                    v * 2 - 9
                } else {
                    v * 2
                }
            } else {
                v
            })
        })
        .sum();
    sum.is_multiple_of(10)
}

/// True when one of the labels appears shortly before `start`.
///
/// Case-insensitive, and tolerant of the `N°`/`:`/newline noise that sits
/// between a label and its value on a real form.
fn labelled(text: &str, start: usize) -> bool {
    let from = text[..start]
        .char_indices()
        .rev()
        .nth(LOOKBEHIND)
        .map_or(0, |(i, _)| i);
    let window = text[from..start].to_ascii_uppercase();
    // EUID belongs here because the European identifier embeds the SIREN
    // verbatim: `FR7501.944681634` is court code + SIREN, and a real Kbis
    // leaked its SIREN through that line while the labelled RCS numbers
    // above it were redacted.
    ["SIRET", "SIREN", "RCS", "EUID"]
        .iter()
        .any(|k| window.contains(k))
}

/// The canonical French SIRET display grouping: 3-3-3-5 with a space or dot
/// between every group. Partial or absent separators fall back to needing a
/// label — conservative on purpose.
static SIRET_GROUPED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{3}[ .]\d{3}[ .]\d{3}[ .]\d{5}$").expect("fixed literal"));

pub struct FrSiret;

impl Recognizer for FrSiret {
    fn entity_type(&self) -> &'static str {
        "FR_SIRET"
    }

    fn analyze(&self, text: &str) -> Vec<Match> {
        SIRET_PATTERN
            .find_iter(text)
            .filter(|m| {
                let d = digits(m.as_str());
                let valid = if d.starts_with(LA_POSTE_SIREN) {
                    d.chars().filter_map(|c| c.to_digit(10)).sum::<u32>() % 5 == 0
                } else {
                    luhn_ok(&d)
                };
                valid && (labelled(text, m.start()) || SIRET_GROUPED.is_match(m.as_str()))
            })
            .map(|m| Match {
                start: m.start(),
                end: m.end(),
                score: 1.0,
            })
            .collect()
    }
}

pub struct FrSiren;

impl Recognizer for FrSiren {
    fn entity_type(&self) -> &'static str {
        "FR_SIREN"
    }

    fn analyze(&self, text: &str) -> Vec<Match> {
        SIREN_PATTERN
            .find_iter(text)
            .filter(|m| {
                let d = digits(m.as_str());
                // No La Poste carve-out here: its SIREN passes Luhn like
                // any other. Only the 14-digit establishment numbers differ.
                luhn_ok(&d) && labelled(text, m.start())
            })
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

    fn siret(t: &str) -> usize {
        FrSiret.analyze(t).len()
    }
    fn siren(t: &str) -> usize {
        FrSiren.analyze(t).len()
    }

    #[test]
    fn a_labelled_siret_is_found() {
        assert_eq!(siret("SIRET 73282932000074"), 1);
        assert_eq!(siret("N° SIRET : 732 829 320 00074"), 1);
    }

    #[test]
    fn an_unlabelled_compact_number_is_ignored_however_valid() {
        // Luhn alone is not evidence. Without a label or the canonical
        // grouping this is just a 14-digit number passing a checksum.
        assert_eq!(siret("Référence 73282932000074"), 0);
        assert_eq!(siren("Total 732829320"), 0);
    }

    #[test]
    fn canonical_grouping_anchors_a_siret_without_any_label() {
        // Verbatim shape from a real CFE form: the SIRET sat in an address
        // block, nearest label 2,758 characters away on a later page. The
        // 3-3-3-5 grouping is itself the anchor.
        assert_eq!(siret("75008 PARIS 944 681 634 00019 66198"), 1);
    }

    #[test]
    fn grouping_does_not_anchor_a_siren_because_amounts_look_like_that() {
        // 732 829 320 passes Luhn AND is exactly how French typesets
        // millions. Redacting it unlabelled would redact money.
        assert_eq!(siren("montant : 732 829 320"), 0);
        assert_eq!(siren("SIREN 732 829 320"), 1);
    }

    #[test]
    fn a_labelled_siren_is_found() {
        assert_eq!(siren("SIREN 732829320"), 1);
        assert_eq!(siren("RCS Paris 732 829 320"), 1);
    }

    #[test]
    fn a_bad_checksum_is_rejected_even_when_labelled() {
        assert_eq!(siret("SIRET 73282932000075"), 0);
        assert_eq!(siren("SIREN 732829321"), 0);
    }

    #[test]
    fn la_poste_siren_needs_no_exception() {
        // Widely described as "the SIREN that fails Luhn". It does not --
        // this assertion is the one that corrected the belief.
        assert!(luhn_ok(LA_POSTE_SIREN));
        assert_eq!(siren("SIREN 356000000"), 1);
    }

    #[test]
    fn a_la_poste_siret_validates_by_its_own_rule() {
        // 35600000009075: digit sum 45, divisible by 5, and it does not
        // satisfy Luhn -- so without the carve-out it would be dropped.
        let d = "35600000009075";
        assert!(!luhn_ok(d), "precondition: La Poste SIRETs fail Luhn");
        assert_eq!(siret(&format!("SIRET {d}")), 1);
    }

    #[test]
    fn the_euid_line_counts_as_a_siren_label() {
        // Verbatim shape from a real Kbis: the EUID is court code + SIREN,
        // so leaving it unredacted republishes the number.
        assert_eq!(siren("Européen - EUID FR7501.944681634"), 1);
    }

    #[test]
    fn a_siret_does_not_also_report_its_leading_siren() {
        // The first nine digits of a SIRET are a valid SIREN, but there is
        // no word boundary after them, so only one match is reported.
        assert_eq!(siren("SIRET 73282932000074"), 0);
    }
}
