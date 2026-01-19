//! Usage:
//! cargo run -p subtitle-fast-comparator --example compare_roi -- \
//!   --yuv-a ./demo/decoder/yuv/00010.yuv --yuv-b ./demo/decoder/yuv/00010.yuv \
//!   --roi-json ./demo/validator/projection/00010.json --comparator sparse-chamfer

use std::env;
use std::error::Error;
use std::path::PathBuf;

use subtitle_fast_comparator::{Backend, Configuration, PreprocessSettings};

#[path = "common/roi_examples.rs"]
mod roi_examples;
use roi_examples::{debug_features, load_frame, load_rois, mask_stats};

struct Args {
    yuv_a: PathBuf,
    yuv_b: PathBuf,
    roi_json: PathBuf,
    comparator: Backend,
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

    let selections = load_rois(&args.roi_json)?;

    let frame_a = load_frame(&args.yuv_a, selections.frame_width, selections.frame_height)?;
    let frame_b = load_frame(&args.yuv_b, selections.frame_width, selections.frame_height)?;

    let preprocess = PreprocessSettings {
        target: selections.luma_band.target,
        delta: selections.luma_band.delta,
    };
    let configuration = Configuration {
        backend: args.comparator,
        preprocess,
    };
    let comparator = configuration.create_comparator();

    println!("Comparator      : {}", args.comparator.as_str());
    println!("YUV A           : {:?}", args.yuv_a);
    println!("YUV B           : {:?}", args.yuv_b);
    println!(
        "Luma band       : target={}, delta={} (from JSON)",
        preprocess.target, preprocess.delta
    );

    for entry in &selections.regions {
        let Some(feature_a) = comparator.extract(&frame_a, &entry.roi) else {
            println!(
                "[{}] skipped: failed to extract features from first frame (ROI may be empty)",
                entry.description
            );
            if let Some((on, total, min, max)) = mask_stats(&frame_a, &entry.roi, preprocess) {
                println!(
                    "    mask coverage={on}/{total} ({:.2}%), luma min/max={:.3}/{:.3}",
                    on as f32 * 100.0 / total as f32,
                    min,
                    max
                );
            }
            if let Some(diag) = debug_features(&frame_a, &entry.roi, preprocess) {
                println!(
                    "    mask(after morph)={}/{} edges={} sampled_points={}",
                    diag.mask_on, diag.mask_total, diag.edge_count, diag.sampled_points
                );
            }
            continue;
        };
        let Some(feature_b) = comparator.extract(&frame_b, &entry.roi) else {
            println!(
                "[{}] skipped: failed to extract features from second frame (ROI may be empty)",
                entry.description
            );
            if let Some((on, total, min, max)) = mask_stats(&frame_b, &entry.roi, preprocess) {
                println!(
                    "    mask coverage={on}/{total} ({:.2}%), luma min/max={:.3}/{:.3}",
                    on as f32 * 100.0 / total as f32,
                    min,
                    max
                );
            }
            if let Some(diag) = debug_features(&frame_b, &entry.roi, preprocess) {
                println!(
                    "    mask(after morph)={}/{} edges={} sampled_points={}",
                    diag.mask_on, diag.mask_total, diag.edge_count, diag.sampled_points
                );
            }
            continue;
        };
        let report = comparator.compare(&feature_a, &feature_b);
        println!(
            "[{}] ROI x={:.4}, y={:.4}, w={:.4}, h={:.4}",
            entry.description, entry.roi.x, entry.roi.y, entry.roi.width, entry.roi.height
        );
        println!(
            "    similarity: {:.4} (same_segment = {})",
            report.similarity, report.same_segment
        );
        if !report.details.is_empty() {
            for metric in &report.details {
                println!("    {:18}: {}", metric.name, format_metric(metric.value));
            }
        }
    }

    Ok(())
}

fn parse_args() -> Result<Args, CliError> {
    let mut yuv_a = PathBuf::from("./demo/decoder/yuv/00010.yuv");
    let mut yuv_b = PathBuf::from("./demo/decoder/yuv/00010.yuv");
    let mut roi_json = PathBuf::from("./demo/validator/projection/00010.json");
    let mut comparator = Backend::SparseChamfer;
    let mut iter = env::args().skip(1);

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--help" | "-h" => return Err(CliError::HelpRequested),
            "--yuv-a" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--yuv-a requires a value".to_string()))?;
                yuv_a = PathBuf::from(value);
            }
            "--yuv-b" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--yuv-b requires a value".to_string()))?;
                yuv_b = PathBuf::from(value);
            }
            "--roi-json" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--roi-json requires a value".to_string()))?;
                roi_json = PathBuf::from(value);
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
        yuv_a,
        yuv_b,
        roi_json,
        comparator,
    })
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  compare_roi [--yuv-a <path>] [--yuv-b <path>] [--roi-json <path>]\n\
       [--comparator <name>]"
    );
}

fn parse_comparator(value: &str) -> Result<Backend, CliError> {
    value
        .parse::<Backend>()
        .map_err(|err| CliError::Message(format!("invalid comparator '{value}': {err}")))
}

fn format_metric(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.2}")
    } else {
        format!("{value:.4}")
    }
}
