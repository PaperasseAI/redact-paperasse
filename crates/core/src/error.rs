use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("ingestion failed: {0}")]
    Ingest(String),

    #[error("redaction failed: {0}")]
    Redact(String),

    #[cfg(feature = "tier-b")]
    #[error("Presidio request failed: {0}")]
    Presidio(#[from] reqwest::Error),

    #[error("unsupported input for this operation: {0}")]
    Unsupported(String),
}
