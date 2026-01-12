use std::error::Error;
use std::path::PathBuf;
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};
use subtitle_fast_ocr::{LumaPlane, NoopOcrEngine, OcrEngine, OcrRegion, OcrRequest};

#[cfg(feature = "engine-ort")]
use subtitle_fast_ocr::OrtOcrEngine;
#[cfg(all(feature = "engine-vision", target_os = "macos"))]
use subtitle_fast_ocr::VisionOcrEngine;

const INPUT_IMAGE: &str = "./demo/rand_cn2.png";
const ITERATIONS: u64 = 100;

fn main() -> Result<(), Box<dyn Error>> {
    let input_path = PathBuf::from(INPUT_IMAGE);
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

    let backends = available_backends();
    if backends.is_empty() {
        return Err("no OCR backends available".into());
    }

    println!(
        "Running OCR benchmark over input {:?} for backends: {:?}",
        input_path, backends
    );

    let mut results = Vec::new();
    for backend in backends {
        println!("\nRunning OCR benchmark for backend='{backend}'...");
        match run_backend_bench(backend, &request) {
            Ok((avg_ms, last_text)) => {
                results.push((backend, avg_ms, last_text));
            }
            Err(err) => {
                eprintln!("backend '{backend}' failed: {err}");
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
            "  {:>12}: avg={avg_ms:.3}ms/{ITERATIONS} calls{suffix}",
            backend
        );
    }

    Ok(())
}

fn available_backends() -> Vec<&'static str> {
    let mut backends = vec!["noop"];
    #[cfg(feature = "engine-ort")]
    backends.push("ort");
    #[cfg(all(feature = "engine-vision", target_os = "macos"))]
    backends.push("vision");
    backends
}

fn run_backend_bench(
    backend: &str,
    request: &OcrRequest<'_>,
) -> Result<(f64, Option<String>), Box<dyn Error>> {
    let engine = build_engine(backend)?;
    engine.warm_up()?;

    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] {prefix:>8.cyan.bold} {bar:40.cyan/blue} {pos:>4}/{len:4} avg={msg}ms",
    )
    .unwrap()
    .progress_chars("█▉▊▋▌▍▎▏  ");
    let bar = ProgressBar::new(ITERATIONS);
    bar.set_style(style);
    bar.set_prefix(engine.name().to_string());
    bar.set_message("0.000");

    let start = Instant::now();
    let mut last_text = None;

    for idx in 0..ITERATIONS {
        let response = engine.recognize(request)?;
        last_text = response.texts.first().map(|entry| entry.text.clone());

        bar.inc(1);
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        let avg_ms = elapsed / (idx + 1) as f64;
        bar.set_message(format!("{avg_ms:.3}"));
    }

    bar.finish_with_message("done");

    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    let avg_ms = elapsed / ITERATIONS as f64;
    Ok((avg_ms, last_text))
}

fn build_engine(name: &str) -> Result<Box<dyn OcrEngine>, Box<dyn Error>> {
    let normalized = name.trim().to_lowercase();
    match normalized.as_str() {
        "ort" => build_ort_engine(),
        "vision" => build_vision_engine(),
        "noop" => Ok(Box::new(NoopOcrEngine)),
        other => Err(format!("unknown OCR backend '{other}'").into()),
    }
}

#[cfg(feature = "engine-ort")]
fn build_ort_engine() -> Result<Box<dyn OcrEngine>, Box<dyn Error>> {
    Ok(Box::new(OrtOcrEngine::new()?))
}

#[cfg(not(feature = "engine-ort"))]
fn build_ort_engine() -> Result<Box<dyn OcrEngine>, Box<dyn Error>> {
    Err("ort backend not available (feature engine-ort disabled)".into())
}

#[cfg(all(feature = "engine-vision", target_os = "macos"))]
fn build_vision_engine() -> Result<Box<dyn OcrEngine>, Box<dyn Error>> {
    Ok(Box::new(VisionOcrEngine::new()?))
}

#[cfg(not(all(feature = "engine-vision", target_os = "macos")))]
fn build_vision_engine() -> Result<Box<dyn OcrEngine>, Box<dyn Error>> {
    Err("vision backend not available on this target".into())
}
