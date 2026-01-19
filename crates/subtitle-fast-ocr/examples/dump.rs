//! Usage:
//! cargo run -p subtitle-fast-ocr --example dump --features engine-all -- \
//!   --input ./demo/rand_cn2.png --backend ort

use std::env;
use std::error::Error;
use std::path::PathBuf;

use subtitle_fast_ocr::{Backend, Configuration, LumaPlane, OcrRegion, OcrRequest};

struct Args {
    input: Option<PathBuf>,
    backend: Backend,
    list_backends: bool,
}

enum CliError {
    HelpRequested,
    Message(String),
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = match parse_args() {
        Ok(args) => args,
        Err(CliError::HelpRequested) => {
            print_usage();
            return Ok(());
        }
        Err(CliError::Message(message)) => {
            eprintln!("{message}");
            print_usage();
            return Err(message.into());
        }
    };

    if args.list_backends {
        print_backends();
        return Ok(());
    }

    let input_path = args
        .input
        .ok_or("missing input path (use --input <path>)")?;
    if !input_path.exists() {
        return Err(format!("input image {:?} does not exist", input_path).into());
    }

    let image = image::open(&input_path)?.to_luma8();
    let (width, height) = image.dimensions();
    let data = image.into_raw();

    let plane = LumaPlane::from_parts(width, height, width as usize, &data)?;
    let region = OcrRegion::new(0.0, 0.0, width as f32, height as f32);
    let regions = vec![region];
    let request = OcrRequest::new(plane, &regions);

    let config = Configuration {
        backend: args.backend,
    };
    let engine = config.create_engine()?;
    engine.warm_up()?;

    let response = engine.recognize(&request)?;

    if response.texts.is_empty() {
        println!("No text recognized in {:?}", input_path);
        return Ok(());
    }

    println!(
        "OCR results for {:?} (backend={}):",
        input_path,
        engine.name()
    );
    for entry in response.texts {
        let conf = entry
            .confidence
            .map(|value| format!("{value:.3}"))
            .unwrap_or_else(|| "n/a".to_string());
        println!(
            "- region=({:.1},{:.1},{:.1},{:.1}) confidence={conf} text='{}'",
            entry.region.x, entry.region.y, entry.region.width, entry.region.height, entry.text
        );
    }

    Ok(())
}

fn parse_args() -> Result<Args, CliError> {
    let mut input = None;
    let mut backend = Backend::Auto;
    let mut list_backends = false;
    let mut iter = env::args().skip(1);

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => return Err(CliError::HelpRequested),
            "--input" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--input requires a value".to_string()))?;
                input = Some(PathBuf::from(value));
            }
            "--backend" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--backend requires a value".to_string()))?;
                backend = parse_backend(&value)?;
            }
            "--list-backends" => {
                list_backends = true;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::Message(format!("unknown flag '{arg}'")));
            }
            _ => {
                if input.is_none() {
                    input = Some(PathBuf::from(arg));
                } else {
                    backend = parse_backend(&arg)?;
                }
            }
        }
    }

    Ok(Args {
        input,
        backend,
        list_backends,
    })
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  dump --input <path> [--backend <name>] [--list-backends]\n\
   (or) dump <path> [backend]"
    );
}

fn print_backends() {
    let backends = Configuration::available_backends();
    if backends.is_empty() {
        println!("No compiled OCR backends available.");
        return;
    }
    println!(
        "Available OCR backends: {}",
        backends
            .iter()
            .map(|backend| backend.as_str())
            .collect::<Vec<&str>>()
            .join(", ")
    );
}

fn parse_backend(value: &str) -> Result<Backend, CliError> {
    value
        .parse::<Backend>()
        .map_err(|err| CliError::Message(format!("invalid backend '{value}': {err}")))
}
