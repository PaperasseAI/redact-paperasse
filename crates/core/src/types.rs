use serde::{Deserialize, Serialize};

/// What goes into the pipeline. `Image` carries only bytes, not a declared
/// media type — nothing downstream trusts a caller-supplied type anyway;
/// `redact_image_bytes` re-derives the real format from content via
/// `image::guess_format`, and every ingestor destructures the bytes and
/// ignores anything else. A caller-declared type that's never checked
/// against the actual content is worse than no field at all.
pub enum Input {
    Text(String),
    Pdf(Vec<u8>),
    Image(Vec<u8>),
    /// A DOCX/XLSX/PPTX/RTF/EPUB/ODT/ODS/ODP/CSV (or legacy .doc/.ppt/.xls)
    /// document — anything anydoc converts to Markdown that isn't a PDF.
    /// `format` selects the parser; `None` auto-detects from content (works
    /// for everything except CSV, which carries no signature and must be
    /// named explicitly — same rule as `anydoc::Format::from_bytes`).
    ///
    /// Only `OutputFormat::Markdown` is meaningful here: anydoc converts
    /// TO markdown, never back to DOCX/XLSX/etc., so there's no "redacted
    /// native document" this pipeline can produce for this variant.
    /// `Engine::process` rejects `OutputFormat::Native` for a `Document`
    /// input rather than silently doing something else.
    Document {
        bytes: Vec<u8>,
        format: Option<DocumentFormat>,
    },
}

/// Mirrors `anydoc::Format` minus `Pdf` (that's `Input::Pdf` instead, since
/// a PDF can also need liteparse's OCR path — a distinction that doesn't
/// apply to any of these formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentFormat {
    Doc,
    Docx,
    Odt,
    Ppt,
    Pptx,
    Rtf,
    Epub,
    Excel,
    Ods,
    Odp,
    Csv,
}

/// What comes out. `Native` (the default) mirrors the input's own shape —
/// redacted image stays an image, redacted PDF stays a PDF, redacted text
/// stays text. `Markdown` forces structured markdown output instead,
/// meaningful for any input type (for Pdf/Image it renders the *extracted*
/// structure post-redaction, not the original pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Native,
    Markdown,
}

/// Viewport-space coordinates (top-left origin, 72 DPI) on a given page —
/// same convention liteparse's `TextItem` uses, since that's the source for
/// this data on the image/PDF ingest path.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BoundingBox {
    pub page: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// A byte-offset range into `ExtractedDocument::text`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionSource {
    /// In-process regex+checksum recognizer (`paperasse-privacy-recognizers`).
    TierA,
    /// Presidio REST call (NER, context-dependent entities).
    TierB,
}

/// A single detected PII entity, ready to redact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    /// Presidio-compatible entity type name (e.g. "FR_NIR", "PERSON").
    pub entity_type: String,
    pub span: Span,
    pub score: f32,
    /// Present only when the source document carries layout info (the
    /// liteparse ingest path); `None` for plain text or anydoc-ingested
    /// documents, which have no page/pixel geometry to redact against.
    pub bbox: Option<BoundingBox>,
    pub source: DetectionSource,
}

/// A single word/text-run with its position, bridging an ingestor's native
/// output (e.g. liteparse's `TextItem`) into a source-agnostic shape the
/// redaction step can consume regardless of which ingestor produced it.
#[derive(Debug, Clone)]
pub struct WordBox {
    /// Byte offset range into the owning `ExtractedDocument::text`.
    pub span: Span,
    pub bbox: BoundingBox,
}

/// Normalized ingest output: whatever the input format was, this is the
/// common shape detection and redaction operate on.
#[derive(Debug, Clone, Default)]
pub struct ExtractedDocument {
    pub text: String,
    /// Structured markdown, when the ingestor produces one (anydoc always
    /// does; liteparse only for its markdown output mode).
    pub markdown: Option<String>,
    /// Empty for anydoc-ingested documents (no layout info) and for plain
    /// text input. Populated on the liteparse (PDF/image) ingest path.
    pub word_boxes: Vec<WordBox>,
    pub page_count: u32,
}

/// The pipeline's final output.
#[derive(Debug, Clone, Default)]
pub struct RedactionResult {
    pub format: OutputFormat,
    pub text: Option<String>,
    pub markdown: Option<String>,
    /// Redacted image/PDF bytes, present when the input (or forced output)
    /// was Image/Pdf and `OutputFormat::Native` was used.
    pub bytes: Option<Vec<u8>>,
    /// Every entity that was found and redacted — an audit trail, not just
    /// a side effect. Callers that need to know *what* was removed (e.g. to
    /// log "1 FR_NIR redacted" without logging the value itself) use this.
    pub entities: Vec<Entity>,
}
