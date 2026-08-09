use async_trait::async_trait;

use crate::error::EngineError;
use crate::types::{BoundingBox, ExtractedDocument, Input, Span, WordBox};

#[async_trait]
pub trait Ingestor: Send + Sync {
    async fn ingest(&self, input: &Input) -> Result<ExtractedDocument, EngineError>;
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
    async fn ingest(&self, input: &Input) -> Result<ExtractedDocument, EngineError> {
        match input {
            Input::Text(text) => Ok(ExtractedDocument {
                text: text.clone(),
                ..Default::default()
            }),
            Input::Pdf(bytes) => {
                // Try anydoc's fast text-layer path first; a scanned/image-only
                // PDF comes back empty (or Unsupported), so fall back to
                // liteparse's PDFium+OCR path — also the only path that
                // yields bounding boxes for redaction placement.
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
    async fn ingest(&self, input: &Input) -> Result<ExtractedDocument, EngineError> {
        let (bytes, format): (&[u8], Option<anydoc::Format>) = match input {
            Input::Text(text) => {
                return Ok(ExtractedDocument {
                    text: text.clone(),
                    ..Default::default()
                });
            }
            Input::Pdf(bytes) => (bytes, Some(anydoc::Format::Pdf)),
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
        // NOTE(build-validation): liteparse's public entry point is typed
        // `PdfInput`, but its own README/flowchart list plain images as a
        // supported input format — presumably routed through the same
        // PDFium conversion step via content-based format detection.
        // Confirm this the first time the workspace builds (see the
        // "install Rust + cargo check" task); split this into its own call
        // if images actually need a distinct entry point.
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
