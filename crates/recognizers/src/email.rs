use std::sync::LazyLock;

use regex::Regex;

use crate::{Match, Recognizer};

/// Recognizes email addresses. No checksum exists for this entity type, so
/// unlike `FrNir` this reports a fixed confidence rather than 1.0/reject —
/// the pattern itself is specific enough to rarely false-positive.
pub struct Email;

static EMAIL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b")
        .expect("EMAIL_ADDRESS pattern is a fixed, tested literal")
});

impl Recognizer for Email {
    fn entity_type(&self) -> &'static str {
        "EMAIL_ADDRESS"
    }

    fn analyze(&self, text: &str) -> Vec<Match> {
        EMAIL_PATTERN
            .find_iter(text)
            .map(|m| Match {
                start: m.start(),
                end: m.end(),
                score: 0.9,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_email_in_text() {
        let matches = Email.analyze("Contact john.smith@example.com for details.");
        assert_eq!(matches.len(), 1);
        assert_eq!(
            &"Contact john.smith@example.com for details."[matches[0].start..matches[0].end],
            "john.smith@example.com"
        );
    }

    #[test]
    fn no_false_positive_on_plain_text() {
        assert!(Email.analyze("no email here").is_empty());
    }
}
