use crate::subtitle_detection::{DEFAULT_DELTA, DEFAULT_TARGET, RoiConfig, SubtitleDetectorKind};

#[derive(Clone, Debug, Default)]
pub struct FrameValidatorConfig {
    pub detection: SubtitleDetectionOptions,
}

#[derive(Clone, Debug)]
pub struct SubtitleDetectionOptions {
    pub enabled: bool,
    pub roi: Option<RoiConfig>,
    pub detector: SubtitleDetectorKind,
    pub luma_band: LumaBandOptions,
}

impl Default for SubtitleDetectionOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            roi: None,
            detector: SubtitleDetectorKind::ProjectionBand,
            luma_band: LumaBandOptions::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LumaBandOptions {
    pub target: u8,
    pub delta: u8,
}

impl Default for LumaBandOptions {
    fn default() -> Self {
        Self {
            target: DEFAULT_TARGET,
            delta: DEFAULT_DELTA,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtitle_detection_options_default_targets_projection_detector() {
        let options = SubtitleDetectionOptions::default();
        assert!(options.enabled);
        assert!(options.roi.is_none());
        assert_eq!(options.detector, SubtitleDetectorKind::ProjectionBand);
        assert_eq!(options.luma_band.target, DEFAULT_TARGET);
        assert_eq!(options.luma_band.delta, DEFAULT_DELTA);
    }

    #[test]
    fn frame_validator_config_default_enables_detection() {
        let config = FrameValidatorConfig::default();
        assert!(config.detection.enabled);
        assert_eq!(
            config.detection.detector,
            SubtitleDetectionOptions::default().detector
        );
    }
}
