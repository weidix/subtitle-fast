use std::sync::Arc;

use crate::config::FrameValidatorConfig;
use crate::detection::SubtitleDetectionPipeline;
use crate::subtitle_detection::RoiConfig;
use crate::subtitle_detection::SubtitleDetectionError;
use crate::subtitle_detection::SubtitleDetectionResult;
use subtitle_fast_types::VideoFrame;

#[derive(Clone)]
/// Validates sampled subtitle frames and optional detection pipelines.
pub struct FrameValidator {
    operations: Arc<ProcessingOperations>,
}

impl FrameValidator {
    pub fn new(config: FrameValidatorConfig) -> Result<Self, SubtitleDetectionError> {
        let operations = ProcessingOperations::new(config);
        Ok(Self {
            operations: Arc::new(operations),
        })
    }

    pub async fn process_frame(
        &self,
        frame: VideoFrame,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        self.process_frame_with_roi(frame, None).await
    }

    pub async fn process_frame_with_roi(
        &self,
        frame: VideoFrame,
        roi: Option<RoiConfig>,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        self.operations.process_frame(frame, roi).await
    }

    pub async fn finalize(&self) {
        self.operations.finalize().await;
    }
}

struct ProcessingOperations {
    detection: Option<Arc<SubtitleDetectionPipeline>>,
}

impl ProcessingOperations {
    fn new(config: FrameValidatorConfig) -> Self {
        let FrameValidatorConfig { detection } = config;
        let detection = SubtitleDetectionPipeline::from_options(detection).map(Arc::new);
        Self { detection }
    }

    async fn process_frame(
        &self,
        frame: VideoFrame,
        roi: Option<RoiConfig>,
    ) -> Result<SubtitleDetectionResult, SubtitleDetectionError> {
        if let Some(pipeline) = self.detection.as_ref() {
            pipeline.process(&frame, roi).await
        } else {
            Ok(SubtitleDetectionResult::empty())
        }
    }

    async fn finalize(&self) {
        if let Some(detection) = self.detection.as_ref() {
            detection.finalize().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FrameValidatorConfig, SubtitleDetectionOptions};
    use subtitle_fast_types::VideoFrame;

    fn sample_frame() -> VideoFrame {
        VideoFrame::from_nv12_owned(8, 8, 8, 8, None, None, vec![0; 64], vec![128; 32])
            .expect("frame")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn disabled_detection_returns_empty_result() {
        let options = SubtitleDetectionOptions {
            enabled: false,
            ..SubtitleDetectionOptions::default()
        };
        let validator =
            FrameValidator::new(FrameValidatorConfig { detection: options }).expect("validator");

        let result = validator
            .process_frame(sample_frame())
            .await
            .expect("result");
        assert!(!result.has_subtitle);
        assert!(result.regions.is_empty());
        validator.finalize().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn processing_operations_without_pipeline_is_empty() {
        let options = SubtitleDetectionOptions {
            enabled: false,
            ..SubtitleDetectionOptions::default()
        };
        let operations = ProcessingOperations::new(FrameValidatorConfig { detection: options });

        let result = operations
            .process_frame(
                sample_frame(),
                Some(RoiConfig {
                    x: 0.1,
                    y: 0.1,
                    width: 0.8,
                    height: 0.8,
                }),
            )
            .await
            .expect("result");
        assert!(!result.has_subtitle);
        assert_eq!(result.max_score, 0.0);
        operations.finalize().await;
    }
}
