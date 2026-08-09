use paperasse_privacy_core::{Engine, Input, OutputFormat};
use pyo3::prelude::*;

/// Redact PII from plain text (Tier A: in-process regex+checksum
/// recognizers, no network call). Pass `markdown=True` to force markdown
/// output.
///
/// Image/PDF bindings land once `redact::redact_image` in the core crate
/// is implemented (currently a stub — see that crate's TODO).
#[pyfunction]
#[pyo3(signature = (text, markdown=false))]
fn redact_text(py: Python<'_>, text: String, markdown: bool) -> PyResult<Bound<'_, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let engine = Engine::default();
        let format = if markdown {
            OutputFormat::Markdown
        } else {
            OutputFormat::Native
        };
        let result = engine
            .process(Input::Text(text), format)
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
