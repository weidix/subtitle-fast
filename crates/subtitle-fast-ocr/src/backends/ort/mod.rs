use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ndarray::{Array4, ArrayD, ArrayView2, Axis};
use ort::session::{Session, builder::GraphOptimizationLevel};
use ort::value::Tensor;

use crate::{OcrEngine, OcrError, OcrRegion, OcrRequest, OcrResponse, OcrText};

const DEFAULT_MODEL_PATH: &str = "models/ch_PP-OCRv5_rec_infer.onnx";
const DEFAULT_DICT_PATH: &str = "models/ch_PP-OCRv5_rec_infer.txt";
const DEFAULT_INPUT_HEIGHT: usize = 48;
const DEFAULT_INPUT_WIDTH: usize = 320;
const DEFAULT_MEAN: f32 = 0.5;
const DEFAULT_STD: f32 = 0.5;

#[derive(Debug, Clone)]
pub struct OrtOcrConfig {
    pub model_path: PathBuf,
    pub dictionary_path: PathBuf,
    pub input_height: usize,
    pub input_width: usize,
    pub normalize_mean: f32,
    pub normalize_std: f32,
}

impl Default for OrtOcrConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::from(DEFAULT_MODEL_PATH),
            dictionary_path: PathBuf::from(DEFAULT_DICT_PATH),
            input_height: DEFAULT_INPUT_HEIGHT,
            input_width: DEFAULT_INPUT_WIDTH,
            normalize_mean: DEFAULT_MEAN,
            normalize_std: DEFAULT_STD,
        }
    }
}

#[derive(Debug)]
pub struct OrtOcrEngine {
    session: Mutex<Session>,
    dictionary: Vec<String>,
    input_height: usize,
    input_width: usize,
    normalize_mean: f32,
    normalize_std: f32,
}

impl OrtOcrEngine {
    pub fn new() -> Result<Self, OcrError> {
        Self::with_config(OrtOcrConfig::default())
    }

    pub fn with_config(config: OrtOcrConfig) -> Result<Self, OcrError> {
        if config.input_height == 0 || config.input_width == 0 {
            return Err(OcrError::backend(
                "ort OCR input dimensions must be non-zero",
            ));
        }
        let dictionary = load_dictionary(&config.dictionary_path)?;
        let session = Session::builder()
            .map_err(|err| OcrError::backend(format!("failed to build ORT session: {err}")))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|err| {
                OcrError::backend(format!("failed to configure ORT optimization: {err}"))
            })?
            .with_intra_threads(2)
            .map_err(|err| OcrError::backend(format!("failed to set ORT threads: {err}")))?
            .commit_from_file(&config.model_path)
            .map_err(|err| {
                OcrError::backend(format!(
                    "failed to load ORT model at {}: {err}",
                    config.model_path.display()
                ))
            })?;

        Ok(Self {
            session: Mutex::new(session),
            dictionary,
            input_height: config.input_height,
            input_width: config.input_width,
            normalize_mean: config.normalize_mean,
            normalize_std: config.normalize_std,
        })
    }
}

impl OcrEngine for OrtOcrEngine {
    fn name(&self) -> &'static str {
        "ort_ppocr"
    }

    fn recognize(&self, request: &OcrRequest<'_>) -> Result<OcrResponse, OcrError> {
        let plane = request.plane();
        if plane.data().is_empty() {
            return Ok(OcrResponse::empty());
        }

        let mut texts = Vec::new();
        for region in request.regions() {
            let Some(crop) = crop_region(
                plane.data(),
                plane.width(),
                plane.height(),
                plane.stride(),
                region,
            ) else {
                continue;
            };

            let input = prepare_input_tensor(
                &crop,
                self.input_height,
                self.input_width,
                self.normalize_mean,
                self.normalize_std,
            )?;

            let tensor = Tensor::from_array(input)
                .map_err(|err| OcrError::backend(format!("failed to build ORT tensor: {err}")))?;

            let output = {
                let mut session = self
                    .session
                    .lock()
                    .map_err(|_| OcrError::backend("ORT session mutex poisoned"))?;
                let outputs = session
                    .run(ort::inputs![tensor])
                    .map_err(|err| OcrError::backend(format!("ORT inference failed: {err}")))?;

                if outputs.len() == 0 {
                    None
                } else {
                    Some(
                        outputs[0]
                            .try_extract_array::<f32>()
                            .map_err(|err| {
                                OcrError::backend(format!("failed to read ORT output: {err}"))
                            })?
                            .to_owned(),
                    )
                }
            };

            let Some(output) = output else {
                continue;
            };

            if let Some((text, confidence)) = decode_output(&output, &self.dictionary) {
                let mut entry = OcrText::new(*region, text);
                if let Some(value) = confidence {
                    entry = entry.with_confidence(value);
                }
                texts.push(entry);
            }
        }

        Ok(OcrResponse::new(texts))
    }
}

struct Crop {
    data: Vec<u8>,
    width: usize,
    height: usize,
}

fn crop_region(
    data: &[u8],
    frame_width: u32,
    frame_height: u32,
    stride: usize,
    region: &OcrRegion,
) -> Option<Crop> {
    let frame_width = frame_width as usize;
    let frame_height = frame_height as usize;
    if frame_width == 0 || frame_height == 0 {
        return None;
    }

    let left = region.x.floor().clamp(0.0, frame_width as f32) as usize;
    let top = region.y.floor().clamp(0.0, frame_height as f32) as usize;
    let right = (region.x + region.width)
        .ceil()
        .clamp(left as f32, frame_width as f32) as usize;
    let bottom = (region.y + region.height)
        .ceil()
        .clamp(top as f32, frame_height as f32) as usize;

    let width = right.saturating_sub(left);
    let height = bottom.saturating_sub(top);
    if width == 0 || height == 0 {
        return None;
    }

    let mut out = Vec::with_capacity(width * height);
    for row in top..bottom {
        let start = row.saturating_mul(stride).saturating_add(left);
        let end = start.saturating_add(width);
        if end > data.len() {
            break;
        }
        out.extend_from_slice(&data[start..end]);
    }

    if out.len() != width * height {
        return None;
    }

    Some(Crop {
        data: out,
        width,
        height,
    })
}

fn prepare_input_tensor(
    crop: &Crop,
    target_height: usize,
    target_width: usize,
    mean: f32,
    std: f32,
) -> Result<Array4<f32>, OcrError> {
    let width = crop.width.max(1);
    let height = crop.height.max(1);
    let scaled_width = ((width as f32 * target_height as f32) / height as f32)
        .ceil()
        .clamp(1.0, target_width as f32) as usize;

    let resized = resize_bilinear(&crop.data, width, height, scaled_width, target_height);
    let mut chw = vec![0.0f32; 3 * target_height * target_width];
    for y in 0..target_height {
        for x in 0..scaled_width {
            let pixel = resized[y * scaled_width + x];
            let value = (pixel / 255.0 - mean) / std;
            for channel in 0..3 {
                let offset = (channel * target_height + y) * target_width + x;
                chw[offset] = value;
            }
        }
    }

    Array4::from_shape_vec((1, 3, target_height, target_width), chw)
        .map_err(|err| OcrError::backend(format!("failed to build OCR input tensor shape: {err}")))
}

fn resize_bilinear(
    src: &[u8],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
) -> Vec<f32> {
    if src_width == dst_width && src_height == dst_height {
        return src.iter().map(|&v| v as f32).collect();
    }

    let mut dst = vec![0.0f32; dst_width * dst_height];
    let scale_x = src_width as f32 / dst_width as f32;
    let scale_y = src_height as f32 / dst_height as f32;

    for y in 0..dst_height {
        let src_y = (y as f32 + 0.5) * scale_y - 0.5;
        let y0 = src_y.floor().clamp(0.0, (src_height - 1) as f32) as usize;
        let y1 = (y0 + 1).min(src_height - 1);
        let wy = src_y - y0 as f32;

        for x in 0..dst_width {
            let src_x = (x as f32 + 0.5) * scale_x - 0.5;
            let x0 = src_x.floor().clamp(0.0, (src_width - 1) as f32) as usize;
            let x1 = (x0 + 1).min(src_width - 1);
            let wx = src_x - x0 as f32;

            let v00 = src[y0 * src_width + x0] as f32;
            let v01 = src[y0 * src_width + x1] as f32;
            let v10 = src[y1 * src_width + x0] as f32;
            let v11 = src[y1 * src_width + x1] as f32;

            let v0 = v00 + (v01 - v00) * wx;
            let v1 = v10 + (v11 - v10) * wx;
            dst[y * dst_width + x] = v0 + (v1 - v0) * wy;
        }
    }

    dst
}

fn load_dictionary(path: &Path) -> Result<Vec<String>, OcrError> {
    let contents = fs::read_to_string(path).map_err(|err| {
        OcrError::backend(format!(
            "failed to read OCR dictionary {}: {err}",
            path.display()
        ))
    })?;
    let mut dictionary = Vec::new();
    for line in contents.lines() {
        if line.is_empty() {
            continue;
        }
        dictionary.push(line.to_string());
    }
    if dictionary.is_empty() {
        return Err(OcrError::backend(format!(
            "OCR dictionary at {} is empty",
            path.display()
        )));
    }
    Ok(dictionary)
}

fn decode_output(output: &ArrayD<f32>, dictionary: &[String]) -> Option<(String, Option<f32>)> {
    let view = output_to_time_major(output, dictionary.len())?;
    let use_probabilities = is_probability_tensor(&view);

    let mut text = String::new();
    let mut prev_idx = usize::MAX;
    let mut confidence_sum = 0.0f32;
    let mut confidence_count = 0u32;
    let dict_len = dictionary.len();

    for row in view.axis_iter(Axis(0)) {
        let (mut best_idx, mut best_val) = (0usize, f32::NEG_INFINITY);
        for (idx, &value) in row.iter().enumerate() {
            if value > best_val {
                best_val = value;
                best_idx = idx;
            }
        }

        let prob = if use_probabilities {
            best_val
        } else {
            softmax_at(&row, best_idx)
        };

        if best_idx != prev_idx && best_idx > 0 && best_idx <= dict_len {
            text.push_str(&dictionary[best_idx - 1]);
            confidence_sum += prob;
            confidence_count = confidence_count.saturating_add(1);
        }
        prev_idx = best_idx;
    }

    if text.is_empty() {
        return None;
    }

    let confidence = if confidence_count > 0 {
        Some(confidence_sum / confidence_count as f32)
    } else {
        None
    };

    Some((text, confidence))
}

fn output_to_time_major<'a>(
    output: &'a ArrayD<f32>,
    dict_len: usize,
) -> Option<ArrayView2<'a, f32>> {
    let shape = output.shape();
    let classes_a = dict_len.saturating_add(1);
    let classes_b = dict_len.saturating_add(2);

    match shape.len() {
        2 => {
            let view = output.view().into_dimensionality::<ndarray::Ix2>().ok()?;
            if shape[1] == classes_a || shape[1] == classes_b {
                Some(view)
            } else if shape[0] == classes_a || shape[0] == classes_b {
                Some(view.reversed_axes())
            } else {
                Some(view)
            }
        }
        3 => {
            let batch = shape[0];
            if batch == 0 {
                return None;
            }
            let view = output
                .index_axis(Axis(0), 0)
                .into_dimensionality::<ndarray::Ix2>()
                .ok()?;
            if shape[2] == classes_a || shape[2] == classes_b {
                Some(view)
            } else if shape[1] == classes_a || shape[1] == classes_b {
                Some(view.reversed_axes())
            } else {
                Some(view)
            }
        }
        _ => None,
    }
}

fn is_probability_tensor(view: &ArrayView2<'_, f32>) -> bool {
    let samples = view.shape()[0].min(3);
    for idx in 0..samples {
        let row = view.index_axis(Axis(0), idx);
        let mut sum = 0.0f32;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for &value in row.iter() {
            sum += value;
            if value < min {
                min = value;
            }
            if value > max {
                max = value;
            }
        }
        if min < -0.05 || max > 1.05 {
            return false;
        }
        if (sum - 1.0).abs() > 0.05 {
            return false;
        }
    }
    true
}

fn softmax_at(row: &ndarray::ArrayView1<'_, f32>, index: usize) -> f32 {
    let mut max = f32::NEG_INFINITY;
    for &value in row.iter() {
        if value > max {
            max = value;
        }
    }
    let mut sum = 0.0f32;
    for &value in row.iter() {
        sum += (value - max).exp();
    }
    if sum == 0.0 {
        return 0.0;
    }
    (row[index] - max).exp() / sum
}
