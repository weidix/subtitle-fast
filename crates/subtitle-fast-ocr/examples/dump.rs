use std::error::Error;
use std::path::PathBuf;

use subtitle_fast_ocr::{LumaPlane, NoopOcrEngine, OcrEngine, OcrRegion, OcrRequest};

#[cfg(feature = "engine-ort")]
use subtitle_fast_ocr::OrtOcrEngine;
#[cfg(all(feature = "engine-vision", target_os = "macos"))]
use subtitle_fast_ocr::VisionOcrEngine;

const INPUT_IMAGE: &str = "./demo/rand_cn2.png";
const OCR_BACKEND: &str = "ort"; // auto | ort | vision | noop

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

    let engine = build_engine(OCR_BACKEND)?;
    engine.warm_up()?;

    let response = engine.recognize(&request)?;

    if response.texts.is_empty() {
        println!("No text recognized in {:?}", input_path);
        return Ok(());
    }

    println!(
        "OCR results for {:?} (backend={}):",
        input_path,
        engine.name()
    );
    for entry in response.texts {
        let conf = entry
            .confidence
            .map(|value| format!("{value:.3}"))
            .unwrap_or_else(|| "n/a".to_string());
        println!(
            "- region=({:.1},{:.1},{:.1},{:.1}) confidence={conf} text='{}'",
            entry.region.x, entry.region.y, entry.region.width, entry.region.height, entry.text
        );
    }

    Ok(())
}

fn build_engine(name: &str) -> Result<Box<dyn OcrEngine>, Box<dyn Error>> {
    let normalized = name.trim().to_lowercase();
    match normalized.as_str() {
        "auto" => build_auto_engine(),
        "ort" => build_ort_engine(),
        "vision" => build_vision_engine(),
        "noop" => Ok(Box::new(NoopOcrEngine)),
        other => Err(format!("unknown OCR backend '{other}'").into()),
    }
}

fn build_auto_engine() -> Result<Box<dyn OcrEngine>, Box<dyn Error>> {
    if let Ok(engine) = build_ort_engine() {
        return Ok(engine);
    }
    if let Ok(engine) = build_vision_engine() {
        return Ok(engine);
    }
    Ok(Box::new(NoopOcrEngine))
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
