use async_trait::async_trait;

use crate::error::EngineError;
use crate::types::{BoundingBox, DocumentFormat, ExtractedDocument, Input, Span, WordBox};

/// Viewport units per image pixel. liteparse reports boxes for a plain
/// image in 72-DPI units under a fixed 150-DPI assumption, regardless of
/// the image's own dimensions — verified empirically by halving an image
/// and observing every coordinate halve exactly. Deliberately NOT tied to
/// `LiteParseConfig::dpi`: that setting only affects PDF page
/// rasterization, so keying off it would misplace boxes on images the
/// moment anyone changed it.
pub(crate) const IMAGE_OCR_DPI: f32 = 150.0;

/// Re-encode `bytes` rotated by an arbitrary `degrees`, padding with white.
///
/// This is for *skew*, not orientation: a page photographed on a desk sits at
/// some incidental angle, and Tesseract copes with a couple of degrees but not
/// with ten. A real URSSAF letter photographed at roughly 12 degrees produced
/// zero detections, while the identical file rotated by 10 or 15 produced the
/// social security number immediately — the pixels were always there, the OCR
/// pass just could not follow the lines.
///
/// White padding, not black or transparent: the corners exposed by rotating
/// become page-coloured rather than a hard edge Tesseract may read as a rule
/// or a character stroke.
pub(crate) fn rotate_bytes_fine(bytes: &[u8], degrees: f32) -> Option<Vec<u8>> {
    use std::io::Cursor;

    let img = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?
        .to_rgb8();
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (sin, cos) = degrees.to_radians().sin_cos();

    // Grow the canvas so no content is clipped off the corners.
    let out_w = (w * cos.abs() + h * sin.abs()).ceil().max(1.0);
    let out_h = (w * sin.abs() + h * cos.abs()).ceil().max(1.0);
    let mut out =
        image::RgbImage::from_pixel(out_w as u32, out_h as u32, image::Rgb([255, 255, 255]));

    // Inverse-map each destination pixel back into the source, so every
    // output pixel is written exactly once and no seams appear.
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ocx, ocy) = (out_w / 2.0, out_h / 2.0);
    for (dx, dy, px) in out.enumerate_pixels_mut() {
        let (rx, ry) = (dx as f32 - ocx, dy as f32 - ocy);
        let sx = rx * cos + ry * sin + cx;
        let sy = -rx * sin + ry * cos + cy;
        if sx >= 0.0 && sy >= 0.0 && sx < w && sy < h {
            *px = *img.get_pixel(sx as u32, sy as u32);
        }
        // else: leave it white — the corners a rotation exposes should read
        // as page, not as a hard edge Tesseract might take for a rule.
    }

    let mut buf = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(out)
        .write_to(&mut buf, image::ImageFormat::Png)
        .ok()?;
    Some(buf.into_inner())
}

/// Re-encode `bytes` rotated clockwise by `degrees` (90/180/270).
///
/// Used to retry OCR on a sideways page. An EXIF tag only helps when the
/// file actually carries one -- messaging apps commonly strip it, scanners
/// often never write it, and a screenshot has none. In those cases the
/// page can be stored sideways with nothing to say so, and a user whose
/// gallery shows it upright has no reason to think anything is wrong.
pub(crate) fn rotate_bytes(bytes: &[u8], degrees: u32) -> Option<Vec<u8>> {
    use std::io::Cursor;
    let img = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let rotated = match degrees {
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        _ => return None,
    };
    let mut out = Cursor::new(Vec::new());
    rotated.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Apply a JPEG/WebP/TIFF's EXIF orientation tag to the pixels, returning
/// re-encoded PNG bytes when a rotation was actually needed.
///
/// Phones store pixels in the sensor's native orientation plus a tag saying
/// how to turn them for display. Every viewer honours that tag, so the photo
/// looks upright to whoever uploaded it -- but `image`'s `decode()` does not
/// apply it, so OCR was being handed sideways text and matched nothing at
/// all. Proven with a pair of otherwise identical files: upright pixels gave
/// 3 detections, the same content stored sideways with Orientation=8 gave 0.
///
/// Returns None when there's nothing to do (no tag, already upright, or the
/// format carries no orientation), so the caller keeps the original bytes.
pub(crate) fn normalize_orientation(bytes: &[u8]) -> Option<Vec<u8>> {
    use image::ImageDecoder;
    use std::io::Cursor;

    let mut decoder = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().ok()?;
    if orientation == image::metadata::Orientation::NoTransforms {
        return None;
    }
    let mut img = image::DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);

    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}
const UNITS_PER_PIXEL: f32 = 72.0 / IMAGE_OCR_DPI;

impl From<DocumentFormat> for anydoc::Format {
    fn from(format: DocumentFormat) -> Self {
        match format {
            DocumentFormat::Doc => anydoc::Format::Doc,
            DocumentFormat::Docx => anydoc::Format::Docx,
            DocumentFormat::Odt => anydoc::Format::Odt,
            DocumentFormat::Ppt => anydoc::Format::Ppt,
            DocumentFormat::Pptx => anydoc::Format::Pptx,
            DocumentFormat::Rtf => anydoc::Format::Rtf,
            DocumentFormat::Epub => anydoc::Format::Epub,
            DocumentFormat::Excel => anydoc::Format::Excel,
            DocumentFormat::Ods => anydoc::Format::Ods,
            DocumentFormat::Odp => anydoc::Format::Odp,
            DocumentFormat::Csv => anydoc::Format::Csv,
        }
    }
}

/// Shared by `DefaultIngestor` and `AnydocIngestor`: both handle
/// `Input::Document` identically (it never routes to liteparse — DOCX/XLSX/
/// etc. rendering-to-boxes isn't something this pipeline supports, see
/// `Input::Document`'s doc comment).
async fn ingest_document(
    bytes: &[u8],
    format: Option<DocumentFormat>,
) -> Result<ExtractedDocument, EngineError> {
    let markdown = anydoc::to_markdown_bytes(bytes, format.map(anydoc::Format::from))
        .map_err(|e| EngineError::Ingest(e.to_string()))?;
    Ok(ExtractedDocument {
        text: markdown.clone(),
        markdown: Some(markdown),
        ..Default::default()
    })
}

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
    async fn ingest(
        &self,
        input: &Input,
        needs_boxes: bool,
    ) -> Result<ExtractedDocument, EngineError>;
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

impl DefaultIngestor {
    /// Use a specific `LiteParseConfig` (e.g. non-default DPI, or an
    /// `ocr_server_url`) instead of `LiteParseConfig::default()`. Prefer
    /// `Engine::with_liteparse_config` over calling this directly — that
    /// threads the same config into `redact_pdf_bytes` too, which needs to
    /// agree with whatever DPI ingestion used.
    pub fn with_liteparse_config(config: liteparse::config::LiteParseConfig) -> Self {
        Self {
            liteparse: LiteparseIngestor::with_config(config),
        }
    }
}

#[async_trait]
impl Ingestor for DefaultIngestor {
    async fn ingest(
        &self,
        input: &Input,
        needs_boxes: bool,
    ) -> Result<ExtractedDocument, EngineError> {
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
            Input::Image(bytes) => self.liteparse.ingest_image(bytes).await,
            Input::Document { bytes, format } => ingest_document(bytes, *format).await,
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
    async fn ingest(
        &self,
        input: &Input,
        needs_boxes: bool,
    ) -> Result<ExtractedDocument, EngineError> {
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
            Input::Image(_) => {
                return Err(EngineError::Unsupported(
                    "anydoc does not read raster images; use the liteparse ingest path".into(),
                ));
            }
            Input::Document { bytes, format } => {
                return ingest_document(bytes, *format).await;
            }
        };
        let markdown = anydoc::to_markdown_bytes(bytes, format)
            .map_err(|e| EngineError::Ingest(e.to_string()))?;
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
#[derive(Default)]
pub struct LiteparseIngestor {
    config: liteparse::config::LiteParseConfig,
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

    /// OCR the image again in an overlapping grid of tiles, and return the
    /// merged result in FULL-IMAGE coordinates.
    ///
    /// Why this exists: Tesseract's page segmentation, not its resolution,
    /// is what loses small print. On a real scanned URSSAF letter carrying
    /// the same NIR twice, a single full-page pass reads the large one and
    /// never emits the small one in the detachable coupon at all — the text
    /// never reaches the recognizer, so no amount of recognizer tuning helps.
    /// The identical pixels, cropped, read fine. Re-running on tiles gives
    /// the segmenter a simpler page each time.
    ///
    /// Tiles overlap so an identifier straddling a cut is still wholly
    /// inside at least one tile. Duplicate detections across the overlap are
    /// expected and harmless for pixel redaction (the same box drawn twice
    /// is the same pixels); `Engine::process` dedupes by box before
    /// reporting.
    pub async fn ingest_image_tiled(
        &self,
        bytes: &[u8],
        grid: u32,
        overlap: f32,
    ) -> Result<ExtractedDocument, EngineError> {
        use std::io::Cursor;

        let decoded = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| EngineError::Ingest(format!("unreadable image: {e}")))?
            .decode()
            .map_err(|e| EngineError::Ingest(format!("failed to decode image: {e}")))?;

        let (w, h) = (decoded.width(), decoded.height());
        let grid = grid.max(1);
        let tile_w = w / grid;
        let tile_h = h / grid;
        let pad_x = (tile_w as f32 * overlap) as u32;
        let pad_y = (tile_h as f32 * overlap) as u32;

        let mut text = String::new();
        let mut word_boxes: Vec<WordBox> = Vec::new();

        for row in 0..grid {
            for col in 0..grid {
                let x0 = (col * tile_w).saturating_sub(pad_x);
                let y0 = (row * tile_h).saturating_sub(pad_y);
                let x1 = ((col + 1) * tile_w + pad_x).min(w);
                let y1 = ((row + 1) * tile_h + pad_y).min(h);
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }

                let tile = decoded.crop_imm(x0, y0, x1 - x0, y1 - y0);
                let mut buf = Cursor::new(Vec::new());
                tile.write_to(&mut buf, image::ImageFormat::Png)
                    .map_err(|e| EngineError::Ingest(format!("failed to encode tile: {e}")))?;

                let Ok(tile_doc) = self.parse(&buf.into_inner()).await else {
                    // One unreadable tile must not sink the whole pass —
                    // the other tiles (and the full-page pass this is
                    // merged with) still contribute.
                    continue;
                };

                // Offsets: liteparse reports image boxes in 72-DPI-equivalent
                // units at a FIXED 150-DPI assumption, independent of the
                // image's own size — verified by halving an image and seeing
                // every coordinate halve exactly. So a tile's pixel origin
                // converts to those units by px * 72/150.
                let base = text.len();
                for wb in tile_doc.word_boxes {
                    let mut b = wb.bbox;
                    b.x += x0 as f32 * UNITS_PER_PIXEL;
                    b.y += y0 as f32 * UNITS_PER_PIXEL;
                    word_boxes.push(WordBox {
                        span: Span {
                            start: base + wb.span.start,
                            end: base + wb.span.end,
                        },
                        bbox: b,
                    });
                }
                text.push_str(&tile_doc.text);
                text.push('\n');
            }
        }

        Ok(ExtractedDocument {
            text,
            markdown: None,
            word_boxes,
            page_count: 1,
        })
    }

    async fn parse(&self, bytes: &[u8]) -> Result<ExtractedDocument, EngineError> {
        use liteparse::types::PdfInput;
        use liteparse::LiteParse;

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
        let mut word_boxes =
            Vec::with_capacity(result.pages.iter().map(|p| p.text_items.len()).sum());
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
    async fn ingest(
        &self,
        input: &Input,
        _needs_boxes: bool,
    ) -> Result<ExtractedDocument, EngineError> {
        match input {
            Input::Text(text) => Ok(ExtractedDocument {
                text: text.clone(),
                ..Default::default()
            }),
            Input::Pdf(bytes) => self.ingest_pdf(bytes).await,
            Input::Image(bytes) => self.ingest_image(bytes).await,
            Input::Document { .. } => Err(EngineError::Unsupported(
                "LiteparseIngestor doesn't handle office-document formats (DOCX/XLSX/PPTX/...) — \
                 that's deliberately anydoc's job (see AnydocIngestor); this pipeline never routes \
                 through liteparse's LibreOffice-based conversion"
                    .into(),
            )),
        }
    }
}

#[cfg(test)]
mod orientation_tests {
    use super::*;

    #[test]
    fn a_plain_image_with_no_exif_is_left_alone() {
        // PNG carries no orientation tag, so this must be a no-op and the
        // caller must keep the original bytes rather than paying a needless
        // decode/re-encode on every upload.
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8));
        let mut buf = std::io::Cursor::new(Vec::new());
        img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        assert!(normalize_orientation(&buf.into_inner()).is_none());
    }

    #[test]
    fn rotating_swaps_the_dimensions_so_a_sideways_page_reads_upright() {
        // The no-EXIF case: a page whose pixels are sideways with no tag to
        // say so. Nothing in the file can tell us, so the engine retries the
        // other orientations when a first pass finds nothing.
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(40, 10)
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("encodes");
        let bytes = png.into_inner();

        let rotated = rotate_bytes(&bytes, 90).expect("rotates");
        let decoded = image::load_from_memory(&rotated).expect("decodes");
        assert_eq!((decoded.width(), decoded.height()), (10, 40));

        let back = rotate_bytes(&rotated, 270).expect("rotates back");
        let decoded = image::load_from_memory(&back).expect("decodes");
        assert_eq!((decoded.width(), decoded.height()), (40, 10));
    }

    #[test]
    fn a_fine_rotation_grows_the_canvas_and_pads_with_page_white() {
        // A skewed page must not be clipped at the corners, and the exposed
        // corners must read as page rather than as a hard edge Tesseract
        // could mistake for a rule.
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(100, 50, image::Rgb([0, 0, 0])))
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("encodes");

        let out = rotate_bytes_fine(&png.into_inner(), 15.0).expect("rotates");
        let img = image::load_from_memory(&out).expect("decodes").to_rgb8();

        assert!(
            img.width() > 100 && img.height() > 50,
            "canvas must grow so nothing is clipped, got {}x{}",
            img.width(),
            img.height()
        );
        assert_eq!(
            *img.get_pixel(0, 0),
            image::Rgb([255, 255, 255]),
            "the corner a rotation exposes must be page-white"
        );
    }

    #[test]
    fn an_unsupported_rotation_is_declined_rather_than_guessed_at() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4)
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("encodes");
        assert!(rotate_bytes(&png.into_inner(), 45).is_none());
    }

    #[test]
    fn garbage_bytes_are_left_alone_rather_than_erroring() {
        // Orientation handling must never be the thing that fails an upload;
        // a real decode error surfaces later with a proper message.
        assert!(normalize_orientation(b"not an image at all").is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DocumentFormat;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn anydoc_extracts_real_csv() {
        // No PDFium, no OCR, no native binary — anydoc's actual (real,
        // not mocked) CSV parser, reachable now that Input::Document
        // exists. This is the capability the whole reason for depending
        // on anydoc rests on; before Input::Document, nothing in this
        // codebase could ever exercise it.
        let csv = b"name,amount\nAlice,10\nBob,20\n".to_vec();
        let result = block_on(AnydocIngestor.ingest(
            &Input::Document {
                bytes: csv,
                format: Some(DocumentFormat::Csv),
            },
            false,
        ))
        .expect("anydoc parses well-formed CSV");
        assert!(result.markdown.is_some());
        let markdown = result.markdown.unwrap();
        assert!(markdown.contains("Alice"));
        assert!(markdown.contains("Bob"));
        assert!(result.word_boxes.is_empty()); // anydoc never produces boxes
    }

    #[test]
    fn anydoc_refuses_pdf_when_boxes_are_needed() {
        // Garbage bytes are fine here: the refusal happens before anydoc
        // ever tries to parse them (see AnydocIngestor::ingest).
        let result = block_on(AnydocIngestor.ingest(&Input::Pdf(vec![0u8; 4]), true));
        assert!(matches!(result, Err(EngineError::Unsupported(_))));
    }

    #[test]
    fn anydoc_refuses_images() {
        let result = block_on(AnydocIngestor.ingest(&Input::Image(vec![0u8; 4]), false));
        assert!(matches!(result, Err(EngineError::Unsupported(_))));
    }

    #[test]
    fn anydoc_text_is_a_passthrough() {
        let result =
            block_on(AnydocIngestor.ingest(&Input::Text("hello world".into()), false)).unwrap();
        assert_eq!(result.text, "hello world");
        assert!(result.markdown.is_none()); // no conversion happened, nothing to report
    }

    #[test]
    fn default_ingestor_text_is_a_passthrough() {
        let result =
            block_on(DefaultIngestor::default().ingest(&Input::Text("hello world".into()), false))
                .unwrap();
        assert_eq!(result.text, "hello world");
    }

    #[test]
    fn default_ingestor_routes_document_through_anydoc() {
        let csv = b"a,b\n1,2\n".to_vec();
        let result = block_on(DefaultIngestor::default().ingest(
            &Input::Document {
                bytes: csv,
                format: Some(DocumentFormat::Csv),
            },
            false,
        ))
        .unwrap();
        assert!(result.markdown.unwrap().contains('1'));
    }

    #[test]
    fn liteparse_ingestor_refuses_document_input() {
        let result = block_on(LiteparseIngestor::default().ingest(
            &Input::Document {
                bytes: vec![],
                format: Some(DocumentFormat::Csv),
            },
            false,
        ));
        assert!(matches!(result, Err(EngineError::Unsupported(_))));
    }
}
