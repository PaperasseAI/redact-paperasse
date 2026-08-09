use serde::{Deserialize, Serialize};

use crate::error::EngineError;
use crate::types::{DetectionSource, Entity, Span};

/// Calls a Presidio deployment's `/analyze` endpoint over REST, for entity
/// types Tier A's regex+checksum recognizers structurally can't cover
/// (names, locations, anything context-dependent rather than a fixed
/// identifier format). Opt-in — enable the `tier-b` feature and construct
/// this only when the extra latency/network hop is worth broader coverage;
/// Tier A stays the default, zero-network path.
///
/// Complements, doesn't duplicate, the `FR_NIR` recognizer added to Presidio
/// on `paperasse-fr-nir` — that recognizer is exactly the kind of
/// checksum-validated identifier this crate's own Tier A ports natively, so
/// running both would be redundant. Point `TierB` at a Presidio deployment
/// for the entities Tier A doesn't have a native recognizer for yet.
pub struct TierB {
    analyzer_url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct AnalyzeRequest<'a> {
    text: &'a str,
    language: &'a str,
}

#[derive(Deserialize)]
struct AnalyzeResponseItem {
    entity_type: String,
    start: usize,
    end: usize,
    score: f32,
}

/// Default request timeout — a synchronous REST hop in an agent's hot path
/// needs a hard bound, not `reqwest`'s default of none at all. Deliberately
/// short: Presidio's `/analyze` is a single in-memory NLP pass over text
/// already in hand, not a job queue: if it hasn't answered in a few
/// seconds, the deployment is unhealthy and waiting longer just delays
/// surfacing that. Use `with_timeout` to override.
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The env var `from_env` reads for the Presidio analyzer's base URL.
pub const ANALYZER_URL_ENV: &str = "PRESIDIO_ANALYZER_URL";

impl TierB {
    /// `analyzer_url` is the base URL of a running presidio-analyzer
    /// deployment, e.g. `http://localhost:5002`.
    pub fn new(analyzer_url: impl Into<String>) -> Self {
        Self {
            analyzer_url: analyzer_url.into(),
            client: reqwest::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .build()
                .expect("static timeout-only client config is always valid"),
        }
    }

    /// Reads the analyzer URL from `PRESIDIO_ANALYZER_URL`, falling back to
    /// `http://localhost:5002` (presidio-analyzer's default port from this
    /// session's own Docker testing) if unset — standard config-from-
    /// environment, not a hardcoded deployment assumption.
    pub fn from_env() -> Self {
        let url =
            std::env::var(ANALYZER_URL_ENV).unwrap_or_else(|_| "http://localhost:5002".to_string());
        Self::new(url)
    }

    /// Override the request timeout (see `DEFAULT_TIMEOUT`'s doc comment
    /// for why there's a default at all).
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("static timeout-only client config is always valid");
        self
    }

    pub async fn analyze(&self, text: &str, language: &str) -> Result<Vec<Entity>, EngineError> {
        let response: Vec<AnalyzeResponseItem> = self
            .client
            .post(format!("{}/analyze", self.analyzer_url))
            .json(&AnalyzeRequest { text, language })
            .send()
            .await?
            .json()
            .await?;

        Ok(response
            .into_iter()
            .map(|item| Entity {
                entity_type: item.entity_type,
                span: Span {
                    start: item.start,
                    end: item.end,
                },
                score: item.score,
                // Tier B only ever sees extracted text, never layout — the
                // caller merging Tier A + Tier B results is responsible for
                // looking up a bbox via the same ExtractedDocument.word_boxes
                // TierA::analyze uses, if pixel redaction is needed.
                bbox: None,
                source: DetectionSource::TierB,
            })
            .collect())
    }
}
