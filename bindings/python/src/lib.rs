use paperasse_privacy_core::{Engine, Input, OutputFormat};
use pyo3::prelude::*;

/// Redact PII from plain text (Tier A: in-process regex+checksum
/// recognizers, no network call). Pass `markdown=True` to force markdown
/// output. Pass `entities=["FR_NIR"]` to redact only that entity type —
/// matches Presidio's `analyzer_entities` filter; omit/`None` to redact
/// every entity type Tier A's recognizers cover.
///
/// Image/PDF redaction (`redact_image`/`redact_pdf`) is planned but not yet
/// exposed at this binding layer; the underlying pipeline
/// (`paperasse-privacy-core`'s `redact_image_bytes`/`redact_pdf_bytes`) is
/// implemented and tested — see that crate.
#[pyfunction]
#[pyo3(signature = (text, markdown=false, entities=None))]
fn redact_text(
    py: Python<'_>,
    text: String,
    markdown: bool,
    entities: Option<Vec<String>>,
) -> PyResult<Bound<'_, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let engine = Engine::default();
        let format = if markdown {
            OutputFormat::Markdown
        } else {
            OutputFormat::Native
        };
        let result = engine
            .process(Input::Text(text), format, entities.as_deref())
            .await
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        Ok(result.text.or(result.markdown).unwrap_or_default())
    })
}

#[pymodule]
fn paperasse_privacy(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(redact_text, m)?)?;
    Ok(())
}
