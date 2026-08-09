use async_trait::async_trait;

use crate::error::EngineError;
use crate::types::{BoundingBox, ExtractedDocument, Input, Span, WordBox};

#[async_trait]
pub trait Ingestor: Send + Sync {
    /// `needs_boxes` is true whenever the caller is going to redact pixels
    /// afterward (`OutputFormat::Native` for a `Pdf`/`Image` input) — it's
    /// not just a performance hint, it changes which library is safe to
    /// use at all. anydoc never produces bounding boxes and is blind to
    /// PII hidden inside an embedded image (it extracts document
    /// structure, not raster content), so it must never be the ingest
    /// path when the output needs pixel-accurate redaction — see
    /// `DefaultIngestor::ingest` for how that's enforced.
    async fn ingest(&self, input: &Input, needs_boxes: bool) -> Result<ExtractedDocument, EngineError>;
}

/// Routes anydoc (office formats + text-based PDFs — no OCR, no bounding
/// boxes, pure Rust, ~5ms) vs. liteparse (scanned/image PDFs and plain
/// images — the only case that needs OCR and the only case that needs
/// bounding boxes for redaction placement) so neither library's weight is
/// paid for a document type it wasn't built for. Never routes through
/// liteparse's LibreOffice-based office-format conversion — anydoc already
/// owns that job, without the native PDFium/Tesseract dependency.
///
/// See `liteparse_anydoc_notes.md` in the design writeup this crate came
/// out of for the full reasoning.
#[derive(Default)]
pub struct DefaultIngestor {
    liteparse: LiteparseIngestor,
}

#[async_trait]
impl Ingestor for DefaultIngestor {
    async fn ingest(&self, input: &Input, needs_boxes: bool) -> Result<ExtractedDocument, EngineError> {
        match input {
            Input::Text(text) => Ok(ExtractedDocument {
                text: text.clone(),
                ..Default::default()
            }),
            Input::Pdf(bytes) => {
                if needs_boxes {
                    // Pixel-accurate output was requested. Always route through
                    // liteparse here, text layer or not: it's the only path that
                    // yields coordinates to place redaction boxes AND the only
                    // path that OCRs PII hidden inside an embedded image (a
                    // scanned ID pasted into an otherwise-typed page) regardless
                    // of whether the surrounding page has its own text layer.
                    // anydoc can't see embedded-image content at all — using it
                    // here would mean such PII is never even detected, not just
                    // never redacted.
                    return self.liteparse.ingest_pdf(bytes).await;
                }
                // No boxes needed (Markdown/text output): anydoc's fast,
                // no-native-binary path is fine whenever there's a real text
                // layer; fall back to liteparse's OCR only when there isn't one.
                match anydoc::to_markdown_bytes(bytes, anydoc::Format::Pdf) {
                    Ok(markdown) if !markdown.trim().is_empty() => Ok(ExtractedDocument {
                        text: markdown.clone(),
                        markdown: Some(markdown),
                        ..Default::default()
                    }),
                    _ => self.liteparse.ingest_pdf(bytes).await,
                }
            }
            Input::Image { bytes, .. } => self.liteparse.ingest_image(bytes).await,
        }
    }
}

/// anydoc, scoped to the formats it uniquely covers well: DOCX/XLSX/PPTX/
/// RTF/EPUB/ODT/CSV, plus text-based PDFs. Use this directly (instead of
/// `DefaultIngestor`) when the caller already knows the document isn't a
/// scan and wants to guarantee the lightweight, no-native-binary path.
pub struct AnydocIngestor;

#[async_trait]
impl Ingestor for AnydocIngestor {
    async fn ingest(&self, input: &Input, needs_boxes: bool) -> Result<ExtractedDocument, EngineError> {
        let (bytes, format): (&[u8], Option<anydoc::Format>) = match input {
            Input::Text(text) => {
                return Ok(ExtractedDocument {
                    text: text.clone(),
                    ..Default::default()
                });
            }
            Input::Pdf(bytes) => {
                if needs_boxes {
                    // See the trait doc comment: anydoc structurally can't
                    // provide boxes and can't see embedded-image content, so
                    // it must refuse rather than silently produce an
                    // unredacted result. Use DefaultIngestor or
                    // LiteparseIngestor directly for pixel-accurate output.
                    return Err(EngineError::Unsupported(
                        "AnydocIngestor cannot produce bounding boxes; it must not be used \
                         when the output needs pixel-level redaction"
                            .into(),
                    ));
                }
                (bytes, Some(anydoc::Format::Pdf))
            }
            Input::Image { .. } => {
                return Err(EngineError::Unsupported(
                    "anydoc does not read raster images; use the liteparse ingest path".into(),
                ));
            }
        };
        let markdown =
            anydoc::to_markdown_bytes(bytes, format).map_err(|e| EngineError::Ingest(e.to_string()))?;
        Ok(ExtractedDocument {
            text: markdown.clone(),
            markdown: Some(markdown),
            ..Default::default()
        })
    }
}

/// liteparse, scoped to the one job anydoc structurally can't do: spatial
/// text + bounding boxes for scanned/image PDFs and plain images, via
/// PDFium + optional OCR (bundled Tesseract by default, or an HTTP OCR
/// server — see `LiteParseConfig::ocr_server_url`).
pub struct LiteparseIngestor {
    config: liteparse::config::LiteParseConfig,
}

impl Default for LiteparseIngestor {
    fn default() -> Self {
        Self {
            config: liteparse::config::LiteParseConfig::default(),
        }
    }
}

impl LiteparseIngestor {
    pub fn with_config(config: liteparse::config::LiteParseConfig) -> Self {
        Self { config }
    }

    pub async fn ingest_pdf(&self, bytes: &[u8]) -> Result<ExtractedDocument, EngineError> {
        self.parse(bytes).await
    }

    pub async fn ingest_image(&self, bytes: &[u8]) -> Result<ExtractedDocument, EngineError> {
        // NOTE(runtime-validation): this compiles — `PdfInput::Bytes` is
        // typed to accept any bytes — but whether liteparse's internal
        // content-based format detection actually routes a plain image
        // through the same PDFium conversion step (per its README/
        // flowchart) rather than erroring is a runtime question a type
        // check can't answer. Verify against a real image once there's a
        // way to run the CLI/tests, not just `cargo check`.
        self.parse(bytes).await
    }

    async fn parse(&self, bytes: &[u8]) -> Result<ExtractedDocument, EngineError> {
        use liteparse::LiteParse;
        use liteparse::types::PdfInput;

        let parser = LiteParse::new(self.config.clone());
        let result = parser
            .parse_input(PdfInput::Bytes(bytes.to_vec()))
            .await
            .map_err(|e| EngineError::Ingest(e.to_string()))?;

        // Build `text` and `word_boxes` from the SAME walk over
        // `text_items`, rather than trying to align our own offsets against
        // liteparse's own `result.text`/`page.text` (whose exact join
        // convention — spacing, newlines — isn't nailed down here). This
        // guarantees span correctness at the cost of losing liteparse's
        // nicer layout-aware text reconstruction; revisit once buildable if
        // the redaction step needs prettier extracted text too.
        let mut text = String::new();
        let mut word_boxes = Vec::with_capacity(result.pages.iter().map(|p| p.text_items.len()).sum());
        for page in &result.pages {
            for item in &page.text_items {
                if !text.is_empty() {
                    text.push(' ');
                }
                let start = text.len();
                text.push_str(&item.text);
                let end = text.len();
                word_boxes.push(WordBox {
                    span: Span { start, end },
                    bbox: BoundingBox {
                        page: page.page_number as u32,
                        x: item.x,
                        y: item.y,
                        width: item.width,
                        height: item.height,
                    },
                });
            }
        }

        Ok(ExtractedDocument {
            text,
            markdown: None,
            word_boxes,
            page_count: result.pages.len() as u32,
        })
    }
}

#[async_trait]
impl Ingestor for LiteparseIngestor {
    /// Ignores `needs_boxes` — this is the path a caller reaches for
    /// specifically because it always produces boxes (and OCRs embedded
    /// images), so there's nothing to change behavior on.
    async fn ingest(&self, input: &Input, _needs_boxes: bool) -> Result<ExtractedDocument, EngineError> {
        match input {
            Input::Text(text) => Ok(ExtractedDocument {
                text: text.clone(),
                ..Default::default()
            }),
            Input::Pdf(bytes) => self.ingest_pdf(bytes).await,
            Input::Image { bytes, .. } => self.ingest_image(bytes).await,
        }
    }
}
