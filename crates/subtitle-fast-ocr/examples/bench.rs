//! Usage:
//! cargo run -p subtitle-fast-ocr --example bench --features engine-all -- \
//!   --input ./demo/rand_cn2.png --iterations 100 --backend ort --backend vision

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};
use subtitle_fast_ocr::{Backend, Configuration, LumaPlane, OcrRegion, OcrRequest};

struct Args {
    input: Option<PathBuf>,
    iterations: u64,
    backends: Vec<Backend>,
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
    if args.iterations == 0 {
        return Err("iterations must be greater than zero".into());
    }

    let image = image::open(&input_path)?.to_luma8();
    let (width, height) = image.dimensions();
    let data = image.into_raw();

    let plane = LumaPlane::from_parts(width, height, width as usize, &data)?;
    let region = OcrRegion::new(0.0, 0.0, width as f32, height as f32);
    let regions = vec![region];
    let request = OcrRequest::new(plane, &regions);

    let available = Configuration::available_backends();
    if available.is_empty() {
        return Err("no OCR backends available".into());
    }

    let backends = if args.backends.is_empty() {
        available
    } else {
        args.backends
    };

    println!(
        "Running OCR benchmark over input {:?} for backends: {:?}",
        input_path,
        backends
            .iter()
            .map(|backend| backend.as_str())
            .collect::<Vec<&str>>()
    );

    let mut results = Vec::new();
    for backend in &backends {
        println!(
            "\nRunning OCR benchmark for backend='{}'...",
            backend.as_str()
        );
        match run_backend_bench(*backend, &request, args.iterations) {
            Ok((avg_ms, last_text)) => {
                results.push((*backend, avg_ms, last_text));
            }
            Err(err) => {
                eprintln!("backend '{}' failed: {err}", backend.as_str());
            }
        }
    }

    if results.is_empty() {
        return Err("no backends produced benchmark results".into());
    }

    println!("\nOCR benchmark summary:");
    for (backend, avg_ms, last_text) in results {
        let suffix = last_text
            .as_deref()
            .map(|text| format!(" text='{text}'"))
            .unwrap_or_default();
        println!(
            "  {:>12}: avg={avg_ms:.3}ms/{} calls{suffix}",
            backend.as_str(),
            args.iterations
        );
    }

    Ok(())
}

fn parse_args() -> Result<Args, CliError> {
    let mut input = None;
    let mut iterations = 100u64;
    let mut backends = Vec::new();
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
            "--iterations" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::Message("--iterations requires a value".to_string())
                })?;
                iterations = value.parse::<u64>().map_err(|_| {
                    CliError::Message("--iterations must be a positive integer".to_string())
                })?;
            }
            "--backend" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--backend requires a value".to_string()))?;
                backends.push(parse_backend(&value)?);
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
                    backends.push(parse_backend(&arg)?);
                }
            }
        }
    }

    Ok(Args {
        input,
        iterations,
        backends,
        list_backends,
    })
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  bench --input <path> [--iterations <n>] [--backend <name>...] [--list-backends]\n\
   (or) bench <path> [backend...]"
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

fn run_backend_bench(
    backend: Backend,
    request: &OcrRequest<'_>,
    iterations: u64,
) -> Result<(f64, Option<String>), Box<dyn Error>> {
    let config = Configuration { backend };
    let engine = config.create_engine()?;
    engine.warm_up()?;

    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>8.cyan.bold} {bar:40.cyan/blue} {pos:>4}/{len:4} avg={msg}ms",
    )
    .unwrap()
    .progress_chars("█▉▊▋▌▍▎▏  ");
    let bar = ProgressBar::new(iterations);
    bar.set_style(style);
    bar.set_prefix(engine.name().to_string());
    bar.set_message("0.000");

    let start = Instant::now();
    let mut last_text = None;

    for idx in 0..iterations {
        let response = engine.recognize(request)?;
        last_text = response.texts.first().map(|entry| entry.text.clone());

        bar.inc(1);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let avg_ms = elapsed / (idx + 1) as f64;
        bar.set_message(format!("{avg_ms:.3}"));
    }

    bar.finish_with_message("done");

    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    let avg_ms = elapsed / iterations as f64;
    Ok((avg_ms, last_text))
}

fn parse_backend(value: &str) -> Result<Backend, CliError> {
    value
        .parse::<Backend>()
        .map_err(|err| CliError::Message(format!("invalid backend '{value}': {err}")))
}
