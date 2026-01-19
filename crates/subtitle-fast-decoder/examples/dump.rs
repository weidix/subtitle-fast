//! Usage:
//! cargo run -p subtitle-fast-decoder --example dump --features backend-all -- \
//!   --input ./demo/video1_30s.mp4 --backend ffmpeg \
//!   --yuv-dir ./demo/decoder/yuv --png-dir ./demo/decoder/png --sample-fps 7

use std::env;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use indicatif::{ProgressBar, ProgressStyle};
use png::{BitDepth, ColorType, Encoder};
use subtitle_fast_decoder::{Backend, Configuration, OutputFormat, VideoFrame};
use tokio_stream::StreamExt;

struct Args {
    input: Option<PathBuf>,
    backend: Option<String>,
    yuv_dir: PathBuf,
    png_dir: PathBuf,
    sample_frequency: usize,
    list_backends: bool,
}

enum CliError {
    HelpRequested,
    Message(String),
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> io::Result<()> {
    let args = match parse_args() {
        Ok(args) => args,
        Err(CliError::HelpRequested) => {
            print_usage();
            return Ok(());
        }
        Err(CliError::Message(message)) => {
            eprintln!("{message}");
            print_usage();
            return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        }
    };

    if args.list_backends {
        print_backends();
        return Ok(());
    }

    let input_path = match args.input {
        Some(path) => path,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing input path (use --input <path>)",
            ));
        }
    };
    if !input_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("input file {:?} does not exist", input_path),
        ));
    }

    let available = Configuration::available_backends();
    if available.is_empty() {
        return Err(io::Error::other(
            "no decoder backend is compiled; enable a backend feature such as backend-ffmpeg",
        ));
    }

    let default_backend = available
        .iter()
        .find(|backend| !matches!(backend, Backend::Mock))
        .copied()
        .unwrap_or(Backend::Mock);

    let backend = if let Some(name) = args.backend {
        Backend::from_str(&name).map_err(|err| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid backend '{name}': {err}"),
            )
        })?
    } else {
        default_backend
    };

    if !available.contains(&backend) {
        return Err(io::Error::other(format!(
            "decoder backend '{}' is not compiled in this build",
            backend.as_str()
        )));
    }

    let yuv_dir = args.yuv_dir;
    let png_dir = args.png_dir;
    std::fs::create_dir_all(&yuv_dir)?;
    std::fs::create_dir_all(&png_dir)?;

    let took = SystemTime::now();
    let config = Configuration {
        backend,
        input: Some(input_path.clone()),
        channel_capacity: None,
        output_format: OutputFormat::Nv12,
        start_frame: None,
    };
    let provider = config.create_provider().map_err(io::Error::other)?;
    let metadata = provider.metadata();
    let total_frames = metadata.total_frames;

    write_metadata(&input_path, backend, args.sample_frequency)?;
    println!("Decoding frames from {:?}", input_path);
    println!("Writing YUV files to {:?}", yuv_dir);
    println!("Writing PNG files to {:?}", png_dir);

    let progress = total_frames.map(|total| {
        let style = ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] {prefix:>10.cyan.bold} \
{bar:40.cyan/blue} {pos:>4}/{len:4} frames",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏  ");
        let bar = ProgressBar::new(total);
        bar.set_style(style);
        bar.set_prefix("decode");
        bar
    });

    let (_controller, mut stream) = provider.open().map_err(io::Error::other)?;
    let mut processed = 0u64;
    let mut current_second: Option<u64> = None;
    let mut emitted_in_second = 0usize;
    while let Some(frame) = stream.next().await {
        match frame {
            Ok(frame) => {
                let ordinal = frame.index().unwrap_or(processed);
                processed += 1;
                if let Some(ref bar) = progress {
                    bar.inc(1);
                } else if processed.is_multiple_of(25) {
                    println!("dumped {processed} frames...");
                }
                if !should_emit_frame(
                    &frame,
                    processed,
                    args.sample_frequency,
                    &mut current_second,
                    &mut emitted_in_second,
                ) {
                    continue;
                }
                write_frame_yuv(&frame, &yuv_dir, ordinal)?;
                write_frame_png(&frame, &png_dir, ordinal)?;
            }
            Err(err) => {
                eprintln!("failed to decode frame: {err}");
                break;
            }
        }
    }
    if let Some(bar) = progress {
        bar.finish_with_message("done");
    }
    let elapsed = took.elapsed().unwrap_or_else(|_| Duration::from_secs(0));
    println!(
        "Wrote {processed} frames to {:?} in {:.2?}",
        PathBuf::from("./demo/decoder"),
        elapsed
    );
    Ok(())
}

fn parse_args() -> Result<Args, CliError> {
    let mut input = None;
    let mut backend = None;
    let mut sample_frequency = 7usize;
    let mut yuv_dir = PathBuf::from("./demo/decoder/yuv");
    let mut png_dir = PathBuf::from("./demo/decoder/png");
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
            "--yuv-dir" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--yuv-dir requires a value".to_string()))?;
                yuv_dir = PathBuf::from(value);
            }
            "--png-dir" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--png-dir requires a value".to_string()))?;
                png_dir = PathBuf::from(value);
            }
            "--sample-fps" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::Message("--sample-fps requires a value".to_string())
                })?;
                sample_frequency = value
                    .parse::<usize>()
                    .map_err(|_| CliError::Message("--sample-fps must be a number".to_string()))?;
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
        yuv_dir,
        png_dir,
        sample_frequency,
        list_backends,
    })
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  dump --input <path> [--backend <name>] [--yuv-dir <dir>] [--png-dir <dir>]\n\
       [--sample-fps <n>] [--list-backends]\n\
   (or) dump <path> [backend]"
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

fn write_metadata(
    input: &Path,
    backend: Backend,
    sample_frequency: usize,
) -> Result<(), io::Error> {
    let mut file = std::fs::File::create("./demo/decoder/decoder_dump.txt")?;
    writeln!(file, "input={}", input.display())?;
    writeln!(file, "backend={}", backend.as_str())?;
    writeln!(file, "sample_frequency_hz={sample_frequency}")?;
    writeln!(file, "generated_at={}", timestamp())?;
    Ok(())
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn write_frame_yuv(frame: &VideoFrame, dir: &Path, index: u64) -> Result<(), io::Error> {
    let file = dir.join(format!("{index:05}.yuv"));
    let data = flatten_nv12(frame);
    std::fs::write(file, data)
}

fn write_frame_png(frame: &VideoFrame, dir: &Path, index: u64) -> Result<(), io::Error> {
    let width = frame.width();
    let height = frame.height();
    let file = std::fs::File::create(dir.join(format!("{index:05}.png")))?;
    let writer = BufWriter::new(file);
    let mut encoder = Encoder::new(writer, width, height);
    encoder.set_color(ColorType::Grayscale);
    encoder.set_depth(BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let data = flatten_y(frame);
    writer.write_image_data(&data)?;
    Ok(())
}

fn flatten_y(frame: &VideoFrame) -> Vec<u8> {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let stride = frame.stride();
    let data = frame.y_plane();
    let mut out = Vec::with_capacity(width * height);
    for row in 0..height {
        let start = row * stride;
        let end = (start + width).min(data.len());
        if end <= start {
            break;
        }
        out.extend_from_slice(&data[start..end]);
        if end - start < width {
            // Unexpected short row, bail early to avoid repeating data.
            break;
        }
    }
    out
}

fn flatten_nv12(frame: &VideoFrame) -> Vec<u8> {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let y_stride = frame.y_stride();
    let uv_stride = frame.uv_stride();
    let y_data = frame.y_plane();
    let uv_data = frame.uv_plane();
    let uv_rows = height.div_ceil(2);
    let mut out = Vec::with_capacity(width * height + width * uv_rows);
    for row in 0..height {
        let start = row * y_stride;
        let end = (start + width).min(y_data.len());
        if end <= start {
            break;
        }
        out.extend_from_slice(&y_data[start..end]);
        if end - start < width {
            break;
        }
    }
    for row in 0..uv_rows {
        let start = row * uv_stride;
        let end = (start + width).min(uv_data.len());
        if end <= start {
            break;
        }
        out.extend_from_slice(&uv_data[start..end]);
        if end - start < width {
            break;
        }
    }
    out
}

fn should_emit_frame(
    frame: &VideoFrame,
    processed: u64,
    sample_frequency: usize,
    current_second: &mut Option<u64>,
    emitted_in_second: &mut usize,
) -> bool {
    if sample_frequency == 0 {
        return true;
    }
    let second_bucket = frame_second_bucket(frame, processed, sample_frequency);
    if current_second.is_none_or(|bucket| bucket != second_bucket) {
        *current_second = Some(second_bucket);
        *emitted_in_second = 0;
    }
    if *emitted_in_second < sample_frequency {
        *emitted_in_second += 1;
        true
    } else {
        false
    }
}

fn frame_second_bucket(frame: &VideoFrame, processed: u64, sample_frequency: usize) -> u64 {
    let seconds = frame
        .pts()
        .map(|ts| ts.as_secs_f64())
        .or_else(|| {
            frame
                .index()
                .or(Some(processed))
                .map(|idx| idx as f64 / sample_frequency.max(1) as f64)
        })
        .unwrap_or(processed as f64 / sample_frequency.max(1) as f64);
    seconds.max(0.0).floor() as u64
}
