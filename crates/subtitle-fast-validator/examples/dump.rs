//! Usage:
//! cargo run -p subtitle-fast-validator --example dump --features detector-vision -- \
//!   --yuv-dir ./demo/decoder/yuv --out-dir ./demo/validator \
//!   --target 235 --delta 12 --detectors integral,projection,vision

use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::thread;

use image::{Rgb, RgbImage};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde_json::json;
use subtitle_fast_types::VideoFrame;
use subtitle_fast_validator::subtitle_detection::{
    Configuration, DetectionRegion, LumaBandConfig, RoiConfig, SubtitleDetectionConfig,
    SubtitleDetectionError, SubtitleDetector, SubtitleDetectorKind,
};

struct Args {
    yuv_dir: PathBuf,
    out_dir: PathBuf,
    target: u8,
    delta: u8,
    detectors: Vec<SubtitleDetectorKind>,
    presets: Vec<(usize, usize)>,
}

enum CliError {
    HelpRequested,
    Message(String),
}

const DIGIT_WIDTH: i32 = 3;
const DIGIT_HEIGHT: i32 = 5;
const LABEL_SCALE: i32 = 5;
const LABEL_SPACING: i32 = LABEL_SCALE;

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

    let total_frames = frames.len();
    let progress = MultiProgress::new();
    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>10.cyan.bold} \
{bar:40.cyan/blue} {pos:>4}/{len:4} frames",
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
        let out_dir = args.out_dir.clone();
        let label = detector_label(kind);

        let bar = progress.add(ProgressBar::new(total_frames as u64));
        bar.set_style(style.clone());
        bar.set_prefix(label.to_string());

        let handle = thread::spawn(move || -> Result<usize, Box<dyn Error + Send + Sync>> {
            let out_dir = out_dir.join(label);
            fs::create_dir_all(&out_dir)?;

            let mut processed = 0usize;
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
                let roi = config.roi;
                let detector = build_dump_detector(kind, config)?;
                let result = detector.detect(&frame)?;

                let mut image = frame_to_image(&frame);
                overlay_regions(&mut image, &result.regions);

                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("frame");
                let out_path = out_dir.join(format!("{stem}.png"));
                image.save(out_path)?;

                let regions_with_index: Vec<_> = result
                    .regions
                    .iter()
                    .enumerate()
                    .map(|(index, region)| {
                        json!({
                            "index": index,
                            "x": region.x,
                            "y": region.y,
                            "width": region.width,
                            "height": region.height,
                            "score": region.score,
                        })
                    })
                    .collect();

                let report = json!({
                    "detector": label,
                    "source": path.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
                    "frame": { "width": width, "height": height },
                    "roi": { "x": roi.x, "y": roi.y, "width": roi.width, "height": roi.height },
                    "luma_band": { "target": target, "delta": delta },
                    "has_subtitle": result.has_subtitle,
                    "max_score": result.max_score,
                    "regions": regions_with_index,
                });
                let json_path = out_dir.join(format!("{stem}.json"));
                fs::write(json_path, serde_json::to_vec_pretty(&report)?)?;

                processed += 1;
                bar.inc(1);
            }

            bar.finish_with_message("done");

            Ok(processed)
        });

        handles.push(handle);
    }

    let mut any_processed = false;
    for handle in handles {
        match handle.join() {
            Ok(Ok(count)) => {
                if count > 0 {
                    any_processed = true;
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

    Ok(())
}

fn parse_args() -> Result<Args, CliError> {
    let mut yuv_dir = PathBuf::from("./demo/decoder/yuv");
    let mut out_dir = PathBuf::from("./demo/validator");
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
            "--out-dir" => {
                let value = iter
                    .next()
                    .ok_or_else(|| CliError::Message("--out-dir requires a value".to_string()))?;
                out_dir = PathBuf::from(value);
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
        out_dir,
        target,
        delta,
        detectors,
        presets,
    })
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!(
        "  dump [--yuv-dir <dir>] [--out-dir <dir>] [--target <u8>] [--delta <u8>]\n\
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

fn detector_label(kind: SubtitleDetectorKind) -> &'static str {
    match kind {
        SubtitleDetectorKind::IntegralBand => "integral",
        SubtitleDetectorKind::ProjectionBand => "projection",
        SubtitleDetectorKind::MacVision => "vision",
        SubtitleDetectorKind::Auto => "auto",
    }
}

fn resolution_from_len(len: usize, presets: &[(usize, usize)]) -> Option<(usize, usize)> {
    presets.iter().copied().find(|(w, h)| {
        let y_len = w * h;
        let uv_rows = h.div_ceil(2);
        let uv_len = w * uv_rows;
        len == y_len || len == y_len + uv_len
    })
}

fn frame_to_image(frame: &VideoFrame) -> RgbImage {
    let width = frame.width();
    let height = frame.height();
    let stride = frame.stride();
    let data = frame.data();
    RgbImage::from_fn(width, height, |x, y| {
        let idx = y as usize * stride + x as usize;
        let v = data[idx];
        Rgb([v, v, v])
    })
}

fn overlay_regions(image: &mut RgbImage, regions: &[DetectionRegion]) {
    for (index, region) in regions.iter().enumerate() {
        draw_box(image, region);
        draw_label(image, region, index);
    }
}

fn draw_box(image: &mut RgbImage, region: &DetectionRegion) {
    let width = image.width() as i32;
    let height = image.height() as i32;

    let left = (region.x * width as f32).round() as i32;
    let top = (region.y * height as f32).round() as i32;
    let right = ((region.x + region.width) * width as f32).round() as i32;
    let bottom = ((region.y + region.height) * height as f32).round() as i32;

    for x in left..right {
        set_pixel(image, x, top);
        set_pixel(image, x, bottom);
    }
    for y in top..bottom {
        set_pixel(image, left, y);
        set_pixel(image, right, y);
    }
}

fn draw_label(image: &mut RgbImage, region: &DetectionRegion, index: usize) {
    let width = image.width() as i32;
    let height = image.height() as i32;

    let left = (region.x * width as f32).round() as i32;
    let top = (region.y * height as f32).round() as i32;

    let text = format!("{index}");
    let label_width = (DIGIT_WIDTH * text.len() as i32 + LABEL_SPACING) * LABEL_SCALE;
    let label_height = DIGIT_HEIGHT * LABEL_SCALE;

    let x0 = (left - label_width - LABEL_SPACING).max(0);
    let y0 = (top - label_height - LABEL_SPACING).max(0);

    for (offset, ch) in text.chars().enumerate() {
        let digit = ch.to_digit(10).unwrap_or(0) as usize;
        let digit_x = x0 + offset as i32 * (DIGIT_WIDTH + 1) * LABEL_SCALE;
        draw_digit(image, digit, digit_x, y0);
    }
}

fn draw_digit(image: &mut RgbImage, digit: usize, x0: i32, y0: i32) {
    let pattern = DIGITS[digit % DIGITS.len()];
    for (y, row) in pattern.iter().enumerate() {
        for (x, &on) in row.iter().enumerate() {
            if on {
                let px = x0 + x as i32 * LABEL_SCALE;
                let py = y0 + y as i32 * LABEL_SCALE;
                for dy in 0..LABEL_SCALE {
                    for dx in 0..LABEL_SCALE {
                        set_pixel(image, px + dx, py + dy);
                    }
                }
            }
        }
    }
}

fn set_pixel(image: &mut RgbImage, x: i32, y: i32) {
    if x < 0 || y < 0 {
        return;
    }
    let (x, y) = (x as u32, y as u32);
    if x < image.width() && y < image.height() {
        image.put_pixel(x, y, Rgb([255, 0, 0]));
    }
}

fn build_dump_detector(
    kind: SubtitleDetectorKind,
    config: SubtitleDetectionConfig,
) -> Result<Box<dyn SubtitleDetector>, SubtitleDetectionError> {
    let configuration = Configuration {
        backend: kind,
        detection: config,
    };
    configuration.create_detector()
}

const DIGITS: [[[bool; DIGIT_WIDTH as usize]; DIGIT_HEIGHT as usize]; 10] = [
    [
        [true, true, true],
        [true, false, true],
        [true, false, true],
        [true, false, true],
        [true, true, true],
    ],
    [
        [false, true, false],
        [true, true, false],
        [false, true, false],
        [false, true, false],
        [true, true, true],
    ],
    [
        [true, true, true],
        [false, false, true],
        [true, true, true],
        [true, false, false],
        [true, true, true],
    ],
    [
        [true, true, true],
        [false, false, true],
        [true, true, true],
        [false, false, true],
        [true, true, true],
    ],
    [
        [true, false, true],
        [true, false, true],
        [true, true, true],
        [false, false, true],
        [false, false, true],
    ],
    [
        [true, true, true],
        [true, false, false],
        [true, true, true],
        [false, false, true],
        [true, true, true],
    ],
    [
        [true, true, true],
        [true, false, false],
        [true, true, true],
        [true, false, true],
        [true, true, true],
    ],
    [
        [true, true, true],
        [false, false, true],
        [false, true, false],
        [false, true, false],
        [false, true, false],
    ],
    [
        [true, true, true],
        [true, false, true],
        [true, true, true],
        [true, false, true],
        [true, true, true],
    ],
    [
        [true, true, true],
        [true, false, true],
        [true, true, true],
        [false, false, true],
        [true, true, true],
    ],
];
