use crate::config::SubtitleDetectionOptions;
use crate::subtitle_detection::{
    LumaBandConfig, RoiConfig, SubtitleDetectionConfig, SubtitleDetectionError,
    SubtitleDetectionResult, SubtitleDetector, SubtitleDetectorKind, build_detector,
};
use std::time::Duration;
use subtitle_fast_types::VideoFrame;
use tokio::sync::Mutex;

static REGION_MARGIN_PX: u32 = 5;
pub(crate) struct SubtitleDetectionPipeline {
    state: Mutex<SubtitleDetectionState>,
    enabled: bool,
}

impl SubtitleDetectionPipeline {
    pub fn from_options(options: SubtitleDetectionOptions) -> Option<Self> {
        if !options.enabled {
            return None;
        }

        Some(Self {
            enabled: options.enabled,
            state: Mutex::new(SubtitleDetectionState::new(options)),
        })
    }

    pub async fn process(
        &self,
        frame: &VideoFrame,
        roi: Option<RoiConfig>,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        let mut detection = if self.enabled {
            let mut state = self.state.lock().await;
            state.process_frame(frame, roi)?
        } else {
            SubtitleDetectionResult::empty()
        };

        if detection.has_subtitle {
            inflate_regions(
                &mut detection,
                frame.width() as usize,
                frame.height() as usize,
                REGION_MARGIN_PX,
            );
        }

        Ok(detection)
    }

    pub async fn finalize(&self) {
        if !self.enabled {
            return;
        }

        let mut state = self.state.lock().await;
        state.finalize();
    }
}

struct SubtitleDetectionState {
    detector: Option<Box<dyn SubtitleDetector>>,
    detector_kind: Option<SubtitleDetectorKind>,
    detector_dims: Option<(usize, usize, usize)>,
    detector_roi: Option<RoiConfig>,
    init_error_logged: bool,
    options: SubtitleDetectionOptions,
}

impl SubtitleDetectionState {
    fn new(options: SubtitleDetectionOptions) -> Self {
        Self {
            detector: None,
            detector_kind: None,
            detector_dims: None,
            detector_roi: None,
            init_error_logged: false,
            options,
        }
    }

    fn process_frame(
        &mut self,
        frame: &VideoFrame,
        roi_override: Option<RoiConfig>,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        if !self.options.enabled {
            return Ok(SubtitleDetectionResult::empty());
        }

        let dims = (
            frame.width() as usize,
            frame.height() as usize,
            frame.stride(),
        );
        let desired_roi = roi_override.or(self.options.roi);
        let detector_kind = self.options.detector;
        let needs_rebuild = self.detector_dims != Some(dims)
            || self.detector_kind != Some(detector_kind)
            || self.detector_roi != desired_roi
            || self.detector.is_none();
        if needs_rebuild {
            self.detector_dims = Some(dims);
            self.detector_kind = Some(detector_kind);
            self.detector_roi = desired_roi;
            let mut detector_config = SubtitleDetectionConfig::for_frame(dims.0, dims.1, dims.2);
            detector_config.luma_band = LumaBandConfig {
                target: self.options.luma_band.target,
                delta: self.options.luma_band.delta,
            };
            if let Some(roi) = desired_roi {
                detector_config.roi = roi;
            }
            match build_detector(detector_kind, detector_config) {
                Ok(detector) => {
                    self.detector = Some(detector);
                    self.init_error_logged = false;
                }
                Err(err) => {
                    if !self.init_error_logged {
                        log_init_failure(detector_kind, &err);
                        self.init_error_logged = true;
                    }
                    self.detector = None;
                    self.detector_kind = None;
                    self.detector_dims = None;
                    self.detector_roi = None;
                    return Err(err);
                }
            }
        }

        let Some(detector) = self.detector.as_ref() else {
            return Ok(SubtitleDetectionResult::empty());
        };

        let frame_index = frame_identifier(frame);

        match detector.detect(frame) {
            Ok(result) => Ok(result),
            Err(err) => {
                eprintln!(
                    "subtitle detection failed for frame {}: {}",
                    frame_index, err
                );
                Err(err)
            }
        }
    }

    fn finalize(&mut self) {
        if !self.options.enabled {
            return;
        }
        self.detector = None;
        self.detector_kind = None;
        self.detector_dims = None;
        self.detector_roi = None;
    }
}

fn inflate_regions(
    result: &mut SubtitleDetectionResult,
    frame_width: usize,
    frame_height: usize,
    margin_px: u32,
) {
    if margin_px == 0 || frame_width == 0 || frame_height == 0 {
        return;
    }
    let margin = margin_px as f32;
    let frame_w = frame_width as f32;
    let frame_h = frame_height as f32;
    for region in &mut result.regions {
        let mut x0 = region.x - margin;
        let mut y0 = region.y - margin;
        let mut x1 = region.x + region.width + margin;
        let mut y1 = region.y + region.height + margin;

        if x0 < 0.0 {
            x0 = 0.0;
        }
        if y0 < 0.0 {
            y0 = 0.0;
        }
        if x1 > frame_w {
            x1 = frame_w;
        }
        if y1 > frame_h {
            y1 = frame_h;
        }

        region.x = x0;
        region.y = y0;
        region.width = (x1 - x0).max(0.0);
        region.height = (y1 - y0).max(0.0);
    }
}

fn frame_identifier(frame: &VideoFrame) -> u64 {
    frame
        .index()
        .or_else(|| frame.pts().map(duration_millis))
        .unwrap_or_default()
}

fn duration_millis(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    if millis > u64::MAX as u128 {
        u64::MAX
    } else {
        millis as u64
    }
}

fn log_init_failure(kind: SubtitleDetectorKind, err: &SubtitleDetectionError) {
    eprintln!(
        "subtitle detection initialization failed for backend '{}': {err}",
        kind.as_str()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use subtitle_fast_types::{DetectionRegion, SubtitleDetectionResult, VideoFrame};

    fn sample_frame(width: u32, height: u32) -> VideoFrame {
        let stride = width as usize;
        let y_len = stride * height as usize;
        let uv_len = stride * (height as usize).div_ceil(2);
        VideoFrame::from_nv12_owned(
            width,
            height,
            stride,
            stride,
            None,
            None,
            vec![0; y_len],
            vec![128; uv_len],
        )
        .expect("frame")
    }

    #[test]
    fn inflate_regions_expands_and_clamps_to_frame() {
        let mut result = SubtitleDetectionResult {
            has_subtitle: true,
            max_score: 0.8,
            regions: vec![DetectionRegion {
                x: 1.0,
                y: 2.0,
                width: 4.0,
                height: 3.0,
                score: 0.8,
            }],
        };

        inflate_regions(&mut result, 10, 8, 5);
        let region = &result.regions[0];
        assert_eq!(region.x, 0.0);
        assert_eq!(region.y, 0.0);
        assert_eq!(region.width, 10.0);
        assert_eq!(region.height, 8.0);
    }

    #[test]
    fn inflate_regions_noop_on_zero_margin_or_dimensions() {
        let original = SubtitleDetectionResult {
            has_subtitle: true,
            max_score: 1.0,
            regions: vec![DetectionRegion {
                x: 2.0,
                y: 3.0,
                width: 4.0,
                height: 5.0,
                score: 0.7,
            }],
        };

        let mut no_margin = original.clone();
        inflate_regions(&mut no_margin, 100, 100, 0);
        assert_eq!(no_margin.regions[0].x, original.regions[0].x);
        assert_eq!(no_margin.regions[0].width, original.regions[0].width);

        let mut zero_width = original.clone();
        inflate_regions(&mut zero_width, 0, 100, 3);
        assert_eq!(zero_width.regions[0].x, original.regions[0].x);
        assert_eq!(zero_width.regions[0].height, original.regions[0].height);
    }

    #[test]
    fn duration_millis_saturates_at_u64_max() {
        let near_max = Duration::from_millis(u64::MAX).saturating_add(Duration::from_secs(1));
        assert_eq!(duration_millis(near_max), u64::MAX);
    }

    #[test]
    fn frame_identifier_prefers_index_then_pts() {
        let mut frame = sample_frame(4, 4);
        frame.set_index(Some(8));
        frame.set_pts(Some(Duration::from_millis(77)));
        assert_eq!(frame_identifier(&frame), 8);

        frame.set_index(None);
        assert_eq!(frame_identifier(&frame), 77);

        frame.set_pts(None);
        assert_eq!(frame_identifier(&frame), 0);
    }
}
