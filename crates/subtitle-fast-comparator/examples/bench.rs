//! Usage:
//! cargo run -p subtitle-fast-comparator --example bench -- \
//!   --yuv-dir ./demo/decoder/yuv --roi-dir ./demo/validator/projection \
//!   --comparators sparse-chamfer,bitset-cover

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use subtitle_fast_comparator::{Backend, Configuration, PreprocessSettings};

#[path = "common/roi_examples.rs"]
mod roi_examples;

use roi_examples::{load_frame, load_rois};

struct Args {
    yuv_dir: PathBuf,
    roi_dir: PathBuf,
    comparators: Vec<Backend>,
}

enum CliError {
    HelpRequested,
    Message(String),
}

#[derive(Debug, Clone, Copy)]
struct BenchStats {
    frames: u64,
    comparisons: u64,
    avg_extract_ms: f64,
    avg_compare_ms: f64,
    avg_total_ms: f64,
    avg_frame_ms: f64,
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

    let json_files = collect_roi_json(&args.roi_dir)?;
    if json_files.is_empty() {
        return Err("no ROI JSON files found for benchmark".into());
    }

    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>10.magenta.bold} \
{bar:40.magenta/blue} {pos:>4}/{len:4} frames {msg}",
    )?
    .progress_chars("█▉▊▋▌▍▎▏  ");
    let multi = MultiProgress::new();

    let mut handles = Vec::new();

    for &kind in &args.comparators {
        let json_files = json_files.clone();
        let bar = multi.add(ProgressBar::new(json_files.len() as u64));
        bar.set_style(style.clone());
        bar.set_prefix(kind.as_str().to_string());
        bar.set_message("roi ext=0.000ms cmp=0.000ms tot=0.000ms");

        let yuv_dir = args.yuv_dir.clone();
        let handle = thread::spawn(move || -> Result<(Backend, BenchStats), String> {
            match run_comparator_bench(&json_files, &yuv_dir, kind, bar) {
                Ok(stats) => Ok((kind, stats)),
                Err(err) => Err(format!("comparator '{}' failed: {err}", kind.as_str())),
            }
        });

        handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in handles {
        match handle.join() {
            Ok(Ok((kind, stats))) => {
                results.push((kind, stats));
            }
            Ok(Err(err)) => {
                eprintln!("comparator worker failed: {err}");
            }
            Err(_) => {
                eprintln!("comparator worker panicked");
            }
        }
    }

    if results.is_empty() {
        return Err("no comparator benchmark results produced".into());
    }

    println!(
        "\nComparator benchmark summary over ROI data in {:?}:",
        args.roi_dir
    );
    for (kind, stats) in results {
        println!(
            "  {:>16}: frames={} comparisons={} per_roi: ext={:.3}ms cmp={:.3}ms tot={:.3}ms per_frame_tot={:.3}ms",
            kind.as_str(),
            stats.frames,
            stats.comparisons,
            stats.avg_extract_ms,
            stats.avg_compare_ms,
            stats.avg_total_ms,
            stats.avg_frame_ms,
        );
    }

    Ok(())
}

fn parse_args() -> Result<Args, CliError> {
    let mut yuv_dir = PathBuf::from("./demo/decoder/yuv");
    let mut roi_dir = PathBuf::from("./demo/validator/projection");
    let mut comparators = Vec::new();
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
            "--comparators" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::Message("--comparators requires a value".to_string())
                })?;
                comparators.extend(parse_comparator_list(&value)?);
            }
            "--comparator" => {
                let value = iter.next().ok_or_else(|| {
                    CliError::Message("--comparator requires a value".to_string())
                })?;
                comparators.push(parse_comparator(&value)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::Message(format!("unknown flag '{arg}'")));
            }
            _ => {
                comparators.push(parse_comparator(&arg)?);
            }
        }
    }

    if comparators.is_empty() {
        comparators = default_comparators();
    }

    Ok(Args {
        yuv_dir,
        roi_dir,
        comparators,
    })
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  bench [--yuv-dir <dir>] [--roi-dir <dir>] [--comparators <list>]\n\
       [--comparator <name>...]"
    );
}

fn parse_comparator_list(value: &str) -> Result<Vec<Backend>, CliError> {
    value
        .split(',')
        .filter(|item| !item.trim().is_empty())
        .map(parse_comparator)
        .collect::<Result<Vec<_>, _>>()
}

fn parse_comparator(value: &str) -> Result<Backend, CliError> {
    value
        .parse::<Backend>()
        .map_err(|err| CliError::Message(format!("invalid comparator '{value}': {err}")))
}

fn default_comparators() -> Vec<Backend> {
    vec![Backend::SparseChamfer, Backend::BitsetCover]
}

fn run_comparator_bench(
    json_files: &[PathBuf],
    yuv_dir: &Path,
    kind: Backend,
    bar: ProgressBar,
) -> Result<BenchStats, Box<dyn Error>> {
    let mut frames = 0u64;
    let mut total_pairs = 0u64;
    let mut total_extract = Duration::from_secs(0);
    let mut total_compare = Duration::from_secs(0);

    for json_path in json_files {
        let stem = json_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or("failed to read JSON file name")?;
        let yuv_path = yuv_dir.join(format!("{stem}.yuv"));
        if !yuv_path.exists() {
            eprintln!("skipping frame {stem}: missing YUV at {:?}", yuv_path);
            bar.inc(1);
            continue;
        }
        frames += 1;

        let selection = load_rois(json_path)?;
        let preprocess = PreprocessSettings {
            target: selection.luma_band.target,
            delta: selection.luma_band.delta,
        };
        let configuration = Configuration {
            backend: kind,
            preprocess,
        };
        let comparator = configuration.create_comparator();

        let frame = load_frame(&yuv_path, selection.frame_width, selection.frame_height)?;

        for entry in &selection.regions {
            let start_extract = Instant::now();
            let Some(feature) = comparator.extract(&frame, &entry.roi) else {
                continue;
            };
            total_extract += start_extract.elapsed();

            let start = Instant::now();
            let _report = comparator.compare(&feature, &feature);
            total_compare += start.elapsed();
            total_pairs += 1;
        }

        bar.inc(1);
        if total_pairs > 0 {
            let avg_extract = total_extract.as_secs_f64() * 1000.0 / total_pairs as f64;
            let avg_compare = total_compare.as_secs_f64() * 1000.0 / total_pairs as f64;
            let avg_total = avg_extract + avg_compare;
            bar.set_message(format!(
                "roi ext={avg_extract:.3}ms cmp={avg_compare:.3}ms tot={avg_total:.3}ms"
            ));
        }
    }

    bar.finish_with_message("done");

    if total_pairs == 0 {
        return Err("no comparisons performed in benchmark".into());
    }

    let total_time = total_extract + total_compare;
    let avg_extract_ms = total_extract.as_secs_f64() * 1000.0 / total_pairs as f64;
    let avg_compare_ms = total_compare.as_secs_f64() * 1000.0 / total_pairs as f64;
    let avg_total_ms = avg_extract_ms + avg_compare_ms;
    let avg_frame_ms = if frames > 0 {
        total_time.as_secs_f64() * 1000.0 / frames as f64
    } else {
        0.0
    };

    Ok(BenchStats {
        frames,
        comparisons: total_pairs,
        avg_extract_ms,
        avg_compare_ms,
        avg_total_ms,
        avg_frame_ms,
    })
}

fn collect_roi_json(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}
