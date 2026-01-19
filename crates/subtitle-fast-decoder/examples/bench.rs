//! Usage:
//! cargo run -p subtitle-fast-decoder --example bench --features backend-all -- \
//!   --input ./demo/video1_30s.mp4

use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};
use subtitle_fast_decoder::{Backend, Configuration, OutputFormat};
use tokio_stream::StreamExt;

struct Args {
    input: Option<PathBuf>,
    include_mock: bool,
    list_backends: bool,
}

enum CliError {
    HelpRequested,
    Message(String),
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
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
        return Err(format!("input file {:?} does not exist", input_path).into());
    }

    let mut backends = Configuration::available_backends();
    if !args.include_mock {
        backends.retain(|backend| !matches!(backend, Backend::Mock));
    }

    if backends.is_empty() {
        return Err(
            "no decoder backend is compiled; enable a backend feature such as backend-ffmpeg"
                .into(),
        );
    }

    let mut results = Vec::new();

    println!(
        "Running decoder benchmark over input {:?} for backends: {:?}",
        input_path,
        backends.iter().map(|b| b.as_str()).collect::<Vec<&str>>()
    );

    for backend in backends {
        println!(
            "\nRunning decoder benchmark for backend='{}'...",
            backend.as_str()
        );
        match run_backend_bench(&input_path, backend).await {
            Ok((frames, avg_ms)) => {
                results.push((backend, frames, avg_ms));
            }
            Err(err) => {
                eprintln!("backend '{}' failed: {err}", backend.as_str());
            }
        }
    }

    if results.is_empty() {
        return Err("no backends produced benchmark results".into());
    }

    println!("\nDecoder benchmark summary over input {:?}:", input_path);
    for (backend, frames, avg_ms) in results {
        println!(
            "  {:>12}: frames={} avg={avg_ms:.3}ms/frame",
            backend.as_str(),
            frames,
        );
    }

    Ok(())
}

fn parse_args() -> Result<Args, CliError> {
    let mut input = None;
    let mut include_mock = false;
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
            "--include-mock" => {
                include_mock = true;
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
                    return Err(CliError::Message(format!("unexpected argument '{arg}'")));
                }
            }
        }
    }

    Ok(Args {
        input,
        include_mock,
        list_backends,
    })
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  bench --input <path> [--include-mock] [--list-backends]\n\
   (or) bench <path>"
    );
}

fn print_backends() {
    let backends = Configuration::available_backends();
    if backends.is_empty() {
        println!("No compiled backends available.");
        return;
    }
    println!(
        "Available backends: {}",
        backends
            .iter()
            .map(|backend| backend.as_str())
            .collect::<Vec<&str>>()
            .join(", ")
    );
}

async fn run_backend_bench(
    input_path: &Path,
    backend: Backend,
) -> Result<(u64, f64), Box<dyn Error>> {
    let config = Configuration {
        backend,
        input: Some(input_path.to_path_buf()),
        channel_capacity: None,
        output_format: OutputFormat::Nv12,
        start_frame: None,
    };

    let provider = config.create_provider()?;
    let metadata = provider.metadata();
    let total_frames = metadata.total_frames;

    let progress = total_frames.map(|total| {
        let style = ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] {prefix:>10.cyan.bold} \
{bar:40.cyan/blue} {pos:>4}/{len:4} frames avg={msg}ms",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏  ");
        let bar = ProgressBar::new(total);
        bar.set_style(style);
        bar.set_prefix(backend.as_str().to_string());
        bar.set_message("0.000");
        bar
    });

    let (_controller, mut stream) = provider.open()?;
    let mut processed = 0u64;
    let bench_start = Instant::now();

    while let Some(item) = stream.next().await {
        match item {
            Ok(_frame) => {
                processed += 1;

                if let Some(ref bar) = progress {
                    bar.inc(1);
                    let elapsed = bench_start.elapsed();
                    let avg_ms = elapsed.as_secs_f64() * 1000.0 / processed as f64;
                    bar.set_message(format!("{avg_ms:.3}"));
                }
            }
            Err(err) => {
                return Err(err.into());
            }
        }
    }

    if let Some(bar) = progress {
        bar.finish_with_message("done");
    }

    if processed == 0 {
        return Err("no frames decoded".into());
    }

    let total = bench_start.elapsed();
    let avg_ms = total.as_secs_f64() * 1000.0 / processed as f64;

    Ok((processed, avg_ms))
}
