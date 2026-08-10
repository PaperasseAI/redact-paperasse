use pyo3::prelude::*;
use redact_paperasse_core::{Engine, Input, OutputFormat};

/// Redact PII from plain text (Tier A: in-process regex+checksum
/// recognizers, no network call). Markdown is the default output (this is
/// a tool for agents, and markdown is what they parse best) — pass
/// `markdown=False` to get back plain text in the input's own shape
/// instead. Pass `entities=["FR_NIR"]` to redact only that entity type —
/// matches Presidio's `analyzer_entities` filter; omit/`None` to redact
/// every entity type Tier A's recognizers cover. Pass
/// `score_threshold=0.95` to drop matches scoring below it — matches
/// Presidio's own `score_threshold`.
#[pyfunction]
#[pyo3(signature = (text, markdown=true, entities=None, score_threshold=None))]
fn redact_text(
    py: Python<'_>,
    text: String,
    markdown: bool,
    entities: Option<Vec<String>>,
    score_threshold: Option<f32>,
) -> PyResult<Bound<'_, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let engine = Engine::default();
        let format = if markdown {
            OutputFormat::Markdown
        } else {
            OutputFormat::Native
        };
        let result = engine
            .process(
                Input::Text(text),
                format,
                entities.as_deref(),
                score_threshold,
            )
            .await
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        // Prefer the field matching what was actually requested. Both
        // fields hold identical content today (`redact_text` sets `text`
        // from the same string as `markdown` on every ingest path), so this
        // is a no-op in practice right now, but it's the correct selection
        // if that ever stops being true.
        let output = if format == OutputFormat::Markdown {
            result.markdown.or(result.text)
        } else {
            result.text.or(result.markdown)
        };
        Ok(output.unwrap_or_default())
    })
}

/// Redact PII from a plain image (jpg/png/…): OCR it (liteparse, bundled
/// Tesseract or a configured HTTP OCR server), find PII via Tier A, and
/// black out each match's bounding box directly on the original pixels.
/// Returns the redacted image bytes, same format as the input. Verified
/// against a real photographed document — see the repo README's "Build
/// status" section.
#[pyfunction]
#[pyo3(signature = (image_bytes, entities=None, score_threshold=None))]
fn redact_image(
    py: Python<'_>,
    image_bytes: Vec<u8>,
    entities: Option<Vec<String>>,
    score_threshold: Option<f32>,
) -> PyResult<Bound<'_, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let engine = Engine::default();
        let result = engine
            .process(
                Input::Image(image_bytes),
                OutputFormat::Native,
                entities.as_deref(),
                score_threshold,
            )
            .await
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        result.bytes.ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("no bytes returned for an image input")
        })
    })
}

/// Redact PII from a PDF: render every page (liteparse/PDFium), find PII
/// via OCR + Tier A, black out matches, and reassemble a new PDF from the
/// redacted page images. Deliberately flattens to an image-based PDF — see
/// `redact_pdf_bytes`'s doc comment in `redact-paperasse-core` for why
/// that's the correct behavior for genuine redaction, not a limitation.
/// Verified against a real document embedded in a PDF — see the repo
/// README's "Build status" section.
#[pyfunction]
#[pyo3(signature = (pdf_bytes, entities=None, score_threshold=None))]
fn redact_pdf(
    py: Python<'_>,
    pdf_bytes: Vec<u8>,
    entities: Option<Vec<String>>,
    score_threshold: Option<f32>,
) -> PyResult<Bound<'_, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let engine = Engine::default();
        let result = engine
            .process(
                Input::Pdf(pdf_bytes),
                OutputFormat::Native,
                entities.as_deref(),
                score_threshold,
            )
            .await
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        result.bytes.ok_or_else(|| {
            pyo3::exceptions::PyRuntimeError::new_err("no bytes returned for a pdf input")
        })
    })
}

/// Redact PII from a plain image (jpg/png/…) and return the OCR'd redacted
/// text directly — no pixel step. `Engine::process`'s `OutputFormat::Markdown`
/// branch runs before the input-type match, so this OCRs via liteparse,
/// finds PII in the OCR'd text via Tier A, and returns the redacted result
/// as text, skipping the bounding-box/pixel-drawing work `redact_image`
/// does. Cheaper than `redact_image` when you only need the text content,
/// not a redacted image to display. No `markdown` parameter: unlike
/// `redact_text`, there's no meaningful "native" alternative to toggle to
/// here -- `OutputFormat::Native` means pixel bytes for an image input, not
/// plain text.
#[pyfunction]
#[pyo3(signature = (image_bytes, entities=None, score_threshold=None))]
fn redact_image_text(
    py: Python<'_>,
    image_bytes: Vec<u8>,
    entities: Option<Vec<String>>,
    score_threshold: Option<f32>,
) -> PyResult<Bound<'_, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let engine = Engine::default();
        let result = engine
            .process(
                Input::Image(image_bytes),
                OutputFormat::Markdown,
                entities.as_deref(),
                score_threshold,
            )
            .await
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        Ok(result.markdown.or(result.text).unwrap_or_default())
    })
}

/// Redact PII from a PDF and return the OCR'd redacted text directly — no
/// pixel step, no page-image reassembly. Same reasoning as
/// `redact_image_text`: `OutputFormat::Markdown` short-circuits
/// `Engine::process` before it reaches the pixel-redaction path.
#[pyfunction]
#[pyo3(signature = (pdf_bytes, entities=None, score_threshold=None))]
fn redact_pdf_text(
    py: Python<'_>,
    pdf_bytes: Vec<u8>,
    entities: Option<Vec<String>>,
    score_threshold: Option<f32>,
) -> PyResult<Bound<'_, PyAny>> {
    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        let engine = Engine::default();
        let result = engine
            .process(
                Input::Pdf(pdf_bytes),
                OutputFormat::Markdown,
                entities.as_deref(),
                score_threshold,
            )
            .await
            .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
        Ok(result.markdown.or(result.text).unwrap_or_default())
    })
}

#[pymodule]
fn redact_paperasse(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(redact_text, m)?)?;
    m.add_function(wrap_pyfunction!(redact_image, m)?)?;
    m.add_function(wrap_pyfunction!(redact_pdf, m)?)?;
    m.add_function(wrap_pyfunction!(redact_image_text, m)?)?;
    m.add_function(wrap_pyfunction!(redact_pdf_text, m)?)?;
    Ok(())
}
