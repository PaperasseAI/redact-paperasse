use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use paperasse_privacy_core::{DocumentFormat, Engine, Input, OutputFormat};

#[derive(Parser)]
#[command(
    name = "ppr",
    about = "Redact PII from images, PDFs, text, and office documents (DOCX/XLSX/PPTX/RTF/EPUB/ODT/CSV/...)"
)]
struct Cli {
    /// File to redact. Reads stdin if omitted.
    file: Option<PathBuf>,

    /// Force the input type instead of guessing from the extension.
    #[arg(long, value_enum)]
    r#as: Option<InputKind>,

    /// Output shape. Defaults to mirroring the input (redacted image stays
    /// an image, etc.); "markdown" forces structured markdown regardless.
    #[arg(long, value_enum, default_value = "native")]
    format: FormatArg,

    /// Where to write the redacted output. Defaults to stdout for
    /// text/markdown, and a sibling `.redacted.<ext>` file for image/pdf.
    #[arg(long)]
    output: Option<PathBuf>,

    /// Print the detected entities (type + span, never the matched text
    /// itself) as JSON to stderr — an audit trail of what was redacted.
    #[arg(long)]
    report: bool,

    /// Only redact these entity types (comma-separated, e.g.
    /// `--entities FR_NIR,EMAIL_ADDRESS`). Matches Presidio's
    /// `analyzer_entities` filter. Defaults to every registered recognizer.
    #[arg(long, value_delimiter = ',')]
    entities: Option<Vec<String>>,

    /// Drop matches scoring below this (0.0-1.0). Matches Presidio's
    /// `score_threshold`. Defaults to no filtering.
    #[arg(long)]
    score_threshold: Option<f32>,

    /// Also run Tier B (a Presidio /analyze REST call) for NER coverage
    /// Tier A's recognizers can't provide — names, locations, anything
    /// context-dependent. Only available with `--as text` (or plain-text
    /// stdin/files): Tier B never has pixel coordinates, so it can't
    /// safely participate in image/PDF redaction yet — this refuses
    /// rather than silently under-redact those. Only present when built
    /// with `--features tier-b`. Reads the analyzer URL from
    /// `PRESIDIO_ANALYZER_URL` (default `http://localhost:5002`).
    #[cfg(feature = "tier-b")]
    #[arg(long)]
    tier_b: bool,

    /// Language code passed to Tier B's Presidio call. Ignored without
    /// `--tier-b`.
    #[cfg(feature = "tier-b")]
    #[arg(long, default_value = "en")]
    language: String,
}

#[derive(Clone, ValueEnum)]
enum InputKind {
    Text,
    Pdf,
    Image,
    Docx,
    Doc,
    Xlsx,
    Ods,
    Pptx,
    Ppt,
    Odt,
    Odp,
    Rtf,
    Epub,
    Csv,
}

impl InputKind {
    /// `None` for Text/Pdf/Image — they're not `Input::Document` variants.
    fn document_format(&self) -> Option<DocumentFormat> {
        Some(match self {
            InputKind::Docx => DocumentFormat::Docx,
            InputKind::Doc => DocumentFormat::Doc,
            InputKind::Xlsx => DocumentFormat::Excel,
            InputKind::Ods => DocumentFormat::Ods,
            InputKind::Pptx => DocumentFormat::Pptx,
            InputKind::Ppt => DocumentFormat::Ppt,
            InputKind::Odt => DocumentFormat::Odt,
            InputKind::Odp => DocumentFormat::Odp,
            InputKind::Rtf => DocumentFormat::Rtf,
            InputKind::Epub => DocumentFormat::Epub,
            InputKind::Csv => DocumentFormat::Csv,
            InputKind::Text | InputKind::Pdf | InputKind::Image => return None,
        })
    }
}

#[derive(Clone, ValueEnum)]
enum FormatArg {
    Native,
    Markdown,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let bytes = match &cli.file {
        Some(path) => std::fs::read(path)?,
        None => {
            use std::io::Read;
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            buf
        }
    };

    let kind = cli
        .r#as
        .clone()
        .unwrap_or_else(|| guess_kind(cli.file.as_deref(), &bytes));
    let input = match kind {
        InputKind::Text => Input::Text(String::from_utf8_lossy(&bytes).into_owned()),
        InputKind::Pdf => Input::Pdf(bytes),
        InputKind::Image => Input::Image(bytes),
        _ => Input::Document {
            bytes,
            format: kind.document_format(),
        },
    };

    let format = match cli.format {
        FormatArg::Native => OutputFormat::Native,
        FormatArg::Markdown => OutputFormat::Markdown,
    };

    #[cfg(feature = "tier-b")]
    let result = if cli.tier_b {
        run_with_tier_b(&cli, input, format).await?
    } else {
        Engine::default()
            .process(input, format, cli.entities.as_deref(), cli.score_threshold)
            .await?
    };
    #[cfg(not(feature = "tier-b"))]
    let result = Engine::default()
        .process(input, format, cli.entities.as_deref(), cli.score_threshold)
        .await?;

    if cli.report {
        eprintln!("{}", serde_json::to_string_pretty(&result.entities)?);
    }

    match (&result.text, &result.markdown, &result.bytes) {
        (Some(text), _, None) => match cli.output {
            Some(path) => std::fs::write(path, text)?,
            None => println!("{text}"),
        },
        (_, _, Some(bytes)) => {
            let path = cli
                .output
                .or_else(|| cli.file.as_ref().map(|f| f.with_extension("redacted.out")))
                .expect("--output is required when redacting from stdin");
            std::fs::write(path, bytes)?;
        }
        _ => unreachable!("Engine::process always sets text or bytes"),
    }

    Ok(())
}

/// The manual ingest → Tier A + Tier B → merge → redact pipeline, composed
/// from paperasse-privacy-core's public pieces instead of `Engine::process`
/// (which only knows about Tier A) — exactly the pattern `TierB`'s own doc
/// comment describes: construct it, run it alongside Tier A, and merge.
///
/// Scoped to `Input::Text` only. Tier B never has pixel coordinates (see
/// `TierB::analyze`'s doc comment), so an image/PDF redaction request with
/// `--tier-b` fails loudly here instead of silently skipping the entities
/// it can't place — the same fail-closed principle `AnydocIngestor`
/// already applies when it can't safely fulfill a pixel-redaction request.
#[cfg(feature = "tier-b")]
async fn run_with_tier_b(
    cli: &Cli,
    input: Input,
    format: OutputFormat,
) -> anyhow::Result<paperasse_privacy_core::RedactionResult> {
    use paperasse_privacy_core::detect::{TierA, TierB};
    use paperasse_privacy_core::redact::redact_text;
    use paperasse_privacy_core::ExtractedDocument;

    let text = match input {
        Input::Text(text) => text,
        _ => anyhow::bail!(
            "--tier-b only supports text input today: Tier B never has pixel coordinates, so it \
             can't safely participate in image/PDF/document redaction (see run_with_tier_b's doc \
             comment). Redact this input without --tier-b, or extract its text first."
        ),
    };

    let doc = ExtractedDocument {
        text: text.clone(),
        ..Default::default()
    };

    let mut entities = TierA::default().analyze(&doc, cli.entities.as_deref(), cli.score_threshold);

    let tier_b_entities = TierB::from_env()
        .analyze(&text, &cli.language)
        .await
        .map_err(|e| anyhow::anyhow!("Tier B (Presidio) request failed: {e}. Set {} to point at a running presidio-analyzer, or drop --tier-b.", paperasse_privacy_core::detect::tier_b::ANALYZER_URL_ENV))?;

    entities.extend(tier_b_entities.into_iter().filter(|e| {
        let entity_ok = cli
            .entities
            .as_deref()
            .is_none_or(|wanted| wanted.iter().any(|w| w == &e.entity_type));
        let score_ok = cli
            .score_threshold
            .is_none_or(|threshold| e.score >= threshold);
        entity_ok && score_ok
    }));

    Ok(redact_text(&doc, &entities, format))
}

fn guess_kind(path: Option<&std::path::Path>, bytes: &[u8]) -> InputKind {
    if bytes.starts_with(b"%PDF") {
        return InputKind::Pdf;
    }
    match path.and_then(|p| p.extension()).and_then(|e| e.to_str()) {
        Some("pdf") => InputKind::Pdf,
        Some("png" | "jpg" | "jpeg" | "gif" | "webp") => InputKind::Image,
        Some("docx" | "docm") => InputKind::Docx,
        Some("doc") => InputKind::Doc,
        Some("xlsx" | "xlsm" | "xlsb" | "xls") => InputKind::Xlsx,
        Some("ods") => InputKind::Ods,
        Some("pptx" | "pptm" | "ppsx" | "ppsm") => InputKind::Pptx,
        Some("ppt" | "pps" | "pot") => InputKind::Ppt,
        Some("odt") => InputKind::Odt,
        Some("odp") => InputKind::Odp,
        Some("rtf") => InputKind::Rtf,
        Some("epub") => InputKind::Epub,
        // CSV has no content signature (anydoc::Format::from_bytes can't
        // detect it either) — extension is the only signal, same rule
        // anydoc itself follows.
        Some("csv") => InputKind::Csv,
        _ => InputKind::Text,
    }
}
