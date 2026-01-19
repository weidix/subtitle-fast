//! Usage:
//! cargo run -p subtitle-fast-comparator --example dump -- \
//!   --yuv-dir ./demo/decoder/yuv --roi-dir ./demo/validator/projection \
//!   --output-dir ./demo/comparator --dump-file ./demo/comparator/comparator_dump.json \
//!   --max-frames 100 --comparator sparse-chamfer

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;
use serde_json::to_writer_pretty;
use subtitle_fast_comparator::{Backend, Configuration, PreprocessSettings};

#[path = "common/roi_examples.rs"]
mod roi_examples;

use roi_examples::{load_frame, load_rois};

struct Args {
    yuv_dir: PathBuf,
    roi_dir: PathBuf,
    output_dir: PathBuf,
    dump_file: Option<PathBuf>,
    max_frames: usize,
    comparator: Backend,
}

enum CliError {
    HelpRequested,
    Message(String),
}

#[derive(Serialize)]
struct RoiResultDump {
    description: String,
    similarity: f32,
    same_segment: bool,
    metrics: BTreeMap<String, f32>,
}

#[derive(Serialize)]
struct FramePairDump {
    prev_frame: String,
    curr_frame: String,
    skipped: Option<String>,
    roi_results: Vec<RoiResultDump>,
}

#[derive(Serialize)]
struct ComparatorDump {
    comparator: String,
    yuv_dir: String,
    roi_dir: String,
    frame_pairs: usize,
    pairs: Vec<FramePairDump>,
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

    let mut frames = collect_yuv_frames(&args.yuv_dir)?;
    if frames.len() < 2 {
        return Err("need at least two YUV frames for comparison".into());
    }

    if args.max_frames > 0 && frames.len() > args.max_frames {
        frames.truncate(args.max_frames);
    }

    let total_pairs = frames.len() - 1;
    let progress = ProgressBar::new(total_pairs as u64);
    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>10.cyan.bold} \
{bar:40.cyan/blue} {pos:>4}/{len:4} frame-pairs",
    )?
    .progress_chars("█▉▊▋▌▍▎▏  ");
    progress.set_style(style);
    progress.set_prefix(args.comparator.as_str().to_string());

    let mut pairs = Vec::new();

    for pair_idx in 1..frames.len() {
        let prev_frame_path = &frames[pair_idx - 1];
        let curr_frame_path = &frames[pair_idx];

        let prev_name = prev_frame_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let curr_name = curr_frame_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();

        let stem = curr_frame_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("failed to read frame file name")?;
        let roi_path = args.roi_dir.join(format!("{stem}.json"));

        if !roi_path.exists() {
            pairs.push(FramePairDump {
                prev_frame: prev_name,
                curr_frame: curr_name,
                skipped: Some(format!("missing ROI JSON at {:?}", roi_path)),
                roi_results: Vec::new(),
            });
            progress.inc(1);
            continue;
        }

        let selection = load_rois(&roi_path)?;
        let preprocess = PreprocessSettings {
            target: selection.luma_band.target,
            delta: selection.luma_band.delta,
        };
        let configuration = Configuration {
            backend: args.comparator,
            preprocess,
        };
        let comparator = configuration.create_comparator();

        let frame_a = load_frame(
            prev_frame_path,
            selection.frame_width,
            selection.frame_height,
        )?;
        let frame_b = load_frame(
            curr_frame_path,
            selection.frame_width,
            selection.frame_height,
        )?;

        let mut roi_results = Vec::new();
        for entry in &selection.regions {
            let Some(feature_a) = comparator.extract(&frame_a, &entry.roi) else {
                continue;
            };
            let Some(feature_b) = comparator.extract(&frame_b, &entry.roi) else {
                continue;
            };

            let report = comparator.compare(&feature_a, &feature_b);
            let metrics = report
                .details
                .iter()
                .map(|metric| (metric.name.to_string(), metric.value))
                .collect::<BTreeMap<_, _>>();
            roi_results.push(RoiResultDump {
                description: entry.description.clone(),
                similarity: report.similarity,
                same_segment: report.same_segment,
                metrics,
            });
        }

        pairs.push(FramePairDump {
            prev_frame: prev_name,
            curr_frame: curr_name,
            skipped: None,
            roi_results,
        });

        progress.inc(1);
    }

    progress.finish_with_message("done");

    fs::create_dir_all(&args.output_dir)?;
    let dump_file = args
        .dump_file
        .unwrap_or_else(|| args.output_dir.join("comparator_dump.json"));
    let file = File::create(&dump_file)?;
    let writer = BufWriter::new(file);

    let dump = ComparatorDump {
        comparator: args.comparator.as_str().to_string(),
        yuv_dir: args.yuv_dir.display().to_string(),
        roi_dir: args.roi_dir.display().to_string(),
        frame_pairs: total_pairs,
        pairs,
    };

    to_writer_pretty(writer, &dump)?;

    println!("Wrote comparator dump to {:?}", dump_file);

    Ok(())
}

fn parse_args() -> Result<Args, CliError> {
    let mut yuv_dir = PathBuf::from("./demo/decoder/yuv");
    let mut roi_dir = PathBuf::from("./demo/validator/projection");
    let mut output_dir = PathBuf::from("./demo/comparator");
    let mut dump_file = None;
    let mut max_frames = 100usize;
    let mut comparator = Backend::SparseChamfer;
    let mut iter = env::args().skip(1);

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => return Err(CliError::HelpRequested),
            "--yuv-dir" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--yuv-dir requires a value".to_string()))?;
                yuv_dir = PathBuf::from(value);
            }
            "--roi-dir" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--roi-dir requires a value".to_string()))?;
                roi_dir = PathBuf::from(value);
            }
            "--output-dir" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::Message("--output-dir requires a value".to_string())
                })?;
                output_dir = PathBuf::from(value);
            }
            "--dump-file" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--dump-file requires a value".to_string()))?;
                dump_file = Some(PathBuf::from(value));
            }
            "--max-frames" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::Message("--max-frames requires a value".to_string())
                })?;
                max_frames = value
                    .parse::<usize>()
                    .map_err(|_| CliError::Message("--max-frames must be a number".to_string()))?;
            }
            "--comparator" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::Message("--comparator requires a value".to_string())
                })?;
                comparator = parse_comparator(&value)?;
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::Message(format!("unknown flag '{arg}'")));
            }
            _ => {
                comparator = parse_comparator(&arg)?;
            }
        }
    }

    Ok(Args {
        yuv_dir,
        roi_dir,
        output_dir,
        dump_file,
        max_frames,
        comparator,
    })
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  dump [--yuv-dir <dir>] [--roi-dir <dir>] [--output-dir <dir>]\n\
       [--dump-file <path>] [--max-frames <n>] [--comparator <name>]"
    );
}

fn parse_comparator(value: &str) -> Result<Backend, CliError> {
    value
        .parse::<Backend>()
        .map_err(|err| CliError::Message(format!("invalid comparator '{value}': {err}")))
}

fn collect_yuv_frames(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut frames = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("yuv") {
            frames.push(path);
        }
    }
    frames.sort();
    Ok(frames)
}
