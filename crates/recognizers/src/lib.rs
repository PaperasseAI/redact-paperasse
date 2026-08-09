//! Tier A: fast, deterministic PII recognizers — regex + checksum only, no ML,
//! no network call, no NLP model. This is the in-process hot path meant to run
//! inside the host language (Node/Python/WASM) with no external dependency.
//!
//! For entity types that need general-purpose NER (names, locations, anything
//! context-dependent rather than a fixed identifier format), see Tier B in
//! `paperasse-privacy-core::detect::tier_b`, which calls out to Presidio.

mod email;
mod fr_nir;

pub use email::Email;
pub use fr_nir::FrNir;

/// A single detected span, in byte offsets into the analyzed text.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub start: usize,
    pub end: usize,
    /// 0.0-1.0. Checksum-validated recognizers (e.g. `FrNir`) report 1.0 for
    /// any match that survives validation — an invalid checksum is filtered
    /// out entirely rather than reported at low confidence.
    pub score: f32,
}

/// A regex+checksum PII recognizer. Implementors should be cheap to construct
/// and safe to share across threads — `default_registry()` builds one of each
/// up front and reuses it for every call.
pub trait Recognizer: Send + Sync {
    /// The entity type name. Matches Presidio's naming convention (e.g.
    /// "FR_NIR", "EMAIL_ADDRESS") so Tier A and Tier B results merge cleanly.
    fn entity_type(&self) -> &'static str;

    /// Find all matches of this entity type in `text`.
    fn analyze(&self, text: &str) -> Vec<Match>;
}

/// The built-in recognizer set. Add new recognizers here as they're written —
/// each one is a self-contained module with its own tests (see `fr_nir.rs`
/// for the pattern: regex + `validate()` + a `#[cfg(test)]` module).
pub fn default_registry() -> Vec<Box<dyn Recognizer>> {
    vec![Box::new(FrNir), Box::new(Email)]
}
