use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use paperasse_privacy_core::{Engine, Input, OutputFormat};

#[derive(Parser)]
#[command(name = "ppr", about = "Redact PII from images, PDFs, and text")]
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
}

#[derive(Clone, ValueEnum)]
enum InputKind {
    Text,
    Pdf,
    Image,
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

    let kind = cli.r#as.unwrap_or_else(|| guess_kind(cli.file.as_deref(), &bytes));
    let input = match kind {
        InputKind::Text => Input::Text(String::from_utf8_lossy(&bytes).into_owned()),
        InputKind::Pdf => Input::Pdf(bytes),
        InputKind::Image => Input::Image {
            bytes,
            media_type: "image/png".to_string(), // TODO: sniff from content
        },
    };

    let format = match cli.format {
        FormatArg::Native => OutputFormat::Native,
        FormatArg::Markdown => OutputFormat::Markdown,
    };

    let engine = Engine::default();
    let result = engine.process(input, format).await?;

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

fn guess_kind(path: Option<&std::path::Path>, bytes: &[u8]) -> InputKind {
    if bytes.starts_with(b"%PDF") {
        return InputKind::Pdf;
    }
    match path.and_then(|p| p.extension()).and_then(|e| e.to_str()) {
        Some("pdf") => InputKind::Pdf,
        Some("png" | "jpg" | "jpeg" | "gif" | "webp") => InputKind::Image,
        _ => InputKind::Text,
    }
}
