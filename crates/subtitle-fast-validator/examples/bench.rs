//! Usage:
//! cargo run -p subtitle-fast-validator --example bench --features detector-vision -- \
//!   --yuv-dir ./demo/decoder/yuv --target 235 --delta 12 \
//!   --detectors integral-band,projection-band,macos-vision

use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use subtitle_fast_types::VideoFrame;
use subtitle_fast_validator::subtitle_detection::{
    Configuration, LumaBandConfig, RoiConfig, SubtitleDetectionConfig, SubtitleDetectionError,
    SubtitleDetector, SubtitleDetectorKind,
};

struct Args {
    yuv_dir: PathBuf,
    target: u8,
    delta: u8,
    detectors: Vec<SubtitleDetectorKind>,
    presets: Vec<(usize, usize)>,
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

    if args.detectors.is_empty() {
        return Err("no detectors selected".into());
    }

    let yuv_dir = args.yuv_dir;
    if !yuv_dir.exists() {
        return Err(format!("missing {:?}", yuv_dir).into());
    }

    let mut frames = Vec::new();
    for entry in fs::read_dir(&yuv_dir)? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("yuv") {
            frames.push(path);
        }
    }
    if frames.is_empty() {
        return Err("no demo frames processed".into());
    }

    let total_frames = frames.len() as u64;
    let multi = MultiProgress::new();
    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>10.magenta.bold} \
{bar:40.magenta/blue} {pos:>4}/{len:4} frames avg={msg}ms",
    )
    .unwrap()
    .progress_chars("█▉▊▋▌▍▎▏  ");

    let mut handles = Vec::new();
    for detector_kind in &args.detectors {
        let frames = frames.clone();
        let kind = *detector_kind;
        let target = args.target;
        let delta = args.delta;
        let presets = args.presets.clone();

        let bar = multi.add(ProgressBar::new(total_frames));
        bar.set_style(style.clone());
        bar.set_prefix(kind.as_str().to_string());
        bar.set_message("0.000");

        let handle = thread::spawn(move || -> Result<(SubtitleDetectorKind, DetectorStats, ProjectionPerf), Box<dyn Error + Send + Sync>> {
            let mut stats = DetectorStats::default();
            let mut projection_perf = ProjectionPerf::default();

            for path in frames {
                let data = fs::read(&path)?;
                let (width, height) = match resolution_from_len(data.len(), &presets) {
                    Some(dim) => dim,
                    None => {
                        eprintln!(
                            "skipping {:?}: unknown resolution ({} bytes)",
                            path,
                            data.len()
                        );
                        continue;
                    }
                };
                let y_len = width * height;
                let uv_rows = height.div_ceil(2);
                let uv_len = width * uv_rows;
                let y_plane = data[..y_len].to_vec();
                let uv_plane = if data.len() >= y_len + uv_len {
                    data[y_len..y_len + uv_len].to_vec()
                } else {
                    vec![128u8; uv_len]
                };
                let frame = VideoFrame::from_nv12_owned(
                    width as u32,
                    height as u32,
                    width,
                    width,
                    None,
                    None,
                    y_plane,
                    uv_plane,
                )?;
                let mut config = SubtitleDetectionConfig::for_frame(width, height, width);
                config.roi = RoiConfig {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                };
                config.luma_band = LumaBandConfig { target, delta };

                let detector = build_bench_detector(kind, config)?;
                let start = Instant::now();
                let _result = detector.detect(&frame)?;
                let duration = start.elapsed();

                stats.record(duration);
                if matches!(kind, SubtitleDetectorKind::ProjectionBand) {
                    projection_perf.record(duration);
                }

                bar.inc(1);
                bar.set_message(format!("{:.3}", stats.avg_ms()));
            }

            bar.finish_with_message("done");

            Ok((kind, stats, projection_perf))
        });

        handles.push(handle);
    }

    let mut any_processed = false;
    let mut stats: HashMap<&'static str, DetectorStats> = HashMap::new();
    let mut projection_perf = ProjectionPerf::default();

    for handle in handles {
        match handle.join() {
            Ok(Ok((kind, detector_stats, worker_projection))) => {
                if detector_stats.frames > 0 {
                    any_processed = true;
                }
                stats.insert(kind.as_str(), detector_stats);

                if matches!(kind, SubtitleDetectorKind::ProjectionBand) {
                    projection_perf.total += worker_projection.total;
                    projection_perf.frames += worker_projection.frames;
                }
            }
            Ok(Err(err)) => {
                return Err(err);
            }
            Err(_) => {
                return Err("detector worker panicked".into());
            }
        }
    }

    if !any_processed {
        return Err("no demo frames processed".into());
    }

    println!("\nBenchmark summary over {total_frames} frames:");
    for detector_kind in &args.detectors {
        if let Some(stat) = stats.get(detector_kind.as_str()) {
            println!(
                "{:>12}: avg={:.3}ms frames={}",
                detector_kind.as_str(),
                stat.avg_ms(),
                stat.frames,
            );
        }
    }
    if projection_perf.frames > 0 {
        projection_perf.print_report();
    }

    Ok(())
}

fn parse_args() -> Result<Args, CliError> {
    let mut yuv_dir = PathBuf::from("./demo/decoder/yuv");
    let mut target = 235u8;
    let mut delta = 12u8;
    let mut detectors = Vec::new();
    let mut presets = Vec::new();
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
            "--target" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--target requires a value".to_string()))?;
                target = value
                    .parse::<u8>()
                    .map_err(|_| CliError::Message("--target must be 0-255".to_string()))?;
            }
            "--delta" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--delta requires a value".to_string()))?;
                delta = value
                    .parse::<u8>()
                    .map_err(|_| CliError::Message("--delta must be 0-255".to_string()))?;
            }
            "--detectors" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--detectors requires a value".to_string()))?;
                detectors.extend(parse_detector_list(&value)?);
            }
            "--detector" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--detector requires a value".to_string()))?;
                detectors.push(parse_detector(&value)?);
            }
            "--preset" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--preset requires a value".to_string()))?;
                presets.push(parse_preset(&value)?);
            }
            _ if arg.starts_with('-') => {
                return Err(CliError::Message(format!("unknown flag '{arg}'")));
            }
            _ => {
                detectors.push(parse_detector(&arg)?);
            }
        }
    }

    if detectors.is_empty() {
        detectors = Configuration::available_backends();
    }
    if presets.is_empty() {
        presets = default_presets();
    }

    Ok(Args {
        yuv_dir,
        target,
        delta,
        detectors,
        presets,
    })
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  bench [--yuv-dir <dir>] [--target <u8>] [--delta <u8>]\n\
       [--detectors <list>] [--detector <name>...] [--preset <WxH>]"
    );
}

fn parse_detector_list(value: &str) -> Result<Vec<SubtitleDetectorKind>, CliError> {
    value
        .split(',')
        .filter(|item| !item.trim().is_empty())
        .map(parse_detector)
        .collect::<Result<Vec<_>, _>>()
}

fn parse_detector(value: &str) -> Result<SubtitleDetectorKind, CliError> {
    value
        .parse::<SubtitleDetectorKind>()
        .map_err(|_| CliError::Message(format!("unknown detector '{value}'")))
}

fn parse_preset(value: &str) -> Result<(usize, usize), CliError> {
    let cleaned = value.trim();
    let (w, h) = cleaned
        .split_once('x')
        .ok_or_else(|| CliError::Message("preset must be in WxH format".to_string()))?;
    let width = w
        .parse::<usize>()
        .map_err(|_| CliError::Message("preset width must be a number".to_string()))?;
    let height = h
        .parse::<usize>()
        .map_err(|_| CliError::Message("preset height must be a number".to_string()))?;
    Ok((width, height))
}

fn default_presets() -> Vec<(usize, usize)> {
    vec![(1920, 1080), (1920, 824)]
}

fn resolution_from_len(len: usize, presets: &[(usize, usize)]) -> Option<(usize, usize)> {
    presets.iter().copied().find(|(w, h)| {
        let y_len = w * h;
        let uv_rows = h.div_ceil(2);
        let uv_len = w * uv_rows;
        len == y_len || len == y_len + uv_len
    })
}

#[derive(Default)]
struct DetectorStats {
    total: Duration,
    frames: u64,
}

impl DetectorStats {
    fn record(&mut self, duration: Duration) {
        self.frames += 1;
        self.total += duration;
    }

    fn avg_ms(&self) -> f64 {
        if self.frames == 0 {
            return 0.0;
        }
        (self.total.as_secs_f64() * 1000.0) / self.frames as f64
    }
}

#[derive(Default)]
struct ProjectionPerf {
    total: Duration,
    frames: u64,
}

impl ProjectionPerf {
    fn record(&mut self, duration: Duration) {
        self.frames += 1;
        self.total += duration;
    }

    fn print_report(&self) {
        if self.frames == 0 {
            return;
        }
        let avg_ms = (self.total.as_secs_f64() * 1000.0) / self.frames as f64;
        eprintln!(
            "[projection][bench-perf] frames={} avg={:.3}ms",
            self.frames, avg_ms
        );
    }
}

fn build_bench_detector(
    kind: SubtitleDetectorKind,
    config: SubtitleDetectionConfig,
) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
    let configuration = Configuration {
        backend: kind,
        detection: config,
    };
    configuration.create_detector()
}
