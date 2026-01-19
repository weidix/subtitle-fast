//! Usage:
//! cargo run -p subtitle-fast-decoder --example video_info --features backend-all -- \
//!   --input ./demo/video1_30s.mp4 --backend ffmpeg

use std::env;
use std::error::Error;
use std::path::PathBuf;
use std::str::FromStr;

use subtitle_fast_decoder::{Backend, Configuration, OutputFormat};

struct Args {
    input: Option<PathBuf>,
    backend: Option<String>,
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
        return Err(format!("input file {:?} does not exist", input_path).into());
    }

    let available = Configuration::available_backends();
    if available.is_empty() {
        return Err(
            "no decoder backend is compiled; enable a backend feature such as backend-ffmpeg"
                .into(),
        );
    }

    let backend = if let Some(name) = args.backend {
        Backend::from_str(&name).map_err(|err| format!("invalid backend '{name}': {err}"))?
    } else {
        available[0]
    };

    if !available.contains(&backend) {
        return Err(format!(
            "decoder backend '{}' is not compiled in this build",
            backend.as_str()
        )
        .into());
    }

    let config = Configuration {
        backend,
        input: Some(input_path),
        channel_capacity: None,
        output_format: OutputFormat::Nv12,
        start_frame: None,
    };

    match config.create_provider() {
        Ok(provider) => {
            let metadata = provider.metadata();

            println!("Backend: {}", backend.as_str());
            println!("Duration: {:?}", metadata.duration);
            println!("FPS: {:?}", metadata.fps);
            println!("Width: {:?}", metadata.width);
            println!("Height: {:?}", metadata.height);
            println!("Total Frames: {:?}", metadata.total_frames);
        }
        Err(err) => {
            eprintln!("Failed to create provider: {err}");
        }
    }

    Ok(())
}

fn parse_args() -> Result<Args, CliError> {
    let mut input = None;
    let mut backend = None;
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
                backend = Some(value);
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
                } else if backend.is_none() {
                    backend = Some(arg);
                } else {
                    return Err(CliError::Message(format!("unexpected argument '{arg}'")));
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
        "  video_info --input <path> [--backend <name>] [--list-backends]\n\
   (or) video_info <path> [backend]"
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
