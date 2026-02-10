pub mod averager;
pub mod detector;
pub mod determiner;
pub mod lifecycle;
pub mod merge;
pub mod ocr;
pub mod sampler;
pub mod sorter;

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use averager::{Averager, AveragerResult};
use detector::Detector;
use futures_util::Stream;
use tokio_stream::wrappers::WatchStream;

#[cfg(feature = "ocr-ort")]
use crate::model;
use crate::settings::{DetectionSettings, EffectiveSettings};
use determiner::{RegionDeterminer, RegionDeterminerError};
use lifecycle::{RegionLifecycleError, RegionLifecycleTracker};
use merge::{Merge, MergeResult};
use ocr::{OcrStageError, SubtitleOcr};
use sampler::FrameSampler;
use sorter::FrameSorter;
use subtitle_fast_decoder::DynDecoderProvider;
#[cfg(all(feature = "ocr-vision", target_os = "macos"))]
use subtitle_fast_ocr::VisionOcrEngine;
use subtitle_fast_ocr::{NoopOcrEngine, OcrEngine};
#[cfg(feature = "ocr-ort")]
use subtitle_fast_ocr::{OcrError, OrtOcrConfig, OrtOcrEngine};
use subtitle_fast_types::DecoderError;
use subtitle_fast_validator::subtitle_detection::SubtitleDetectionError;

pub use crate::subtitle::{
    MergedSubtitle, SubtitleLine, TimedSubtitle, render_srt, sort_subtitles,
};
pub use merge::{SubtitleStats, SubtitleUpdate, SubtitleUpdateKind};

pub struct StreamBundle<T> {
    pub stream: Pin<Box<dyn Stream<Item = T> + Send>>,
    pub total_frames: Option<u64>,
}

impl<T> StreamBundle<T> {
    pub fn new(stream: Pin<Box<dyn Stream<Item = T> + Send>>, total_frames: Option<u64>) -> Self {
        Self {
            stream,
            total_frames,
        }
    }
}

#[derive(Clone)]
pub struct PipelineConfig {
    pub detection: DetectionSettings,
    pub ocr: OcrPipelineConfig,
    pub output: OutputPipelineConfig,
}

#[derive(Clone)]
pub struct OcrPipelineConfig {
    pub engine: Arc<dyn OcrEngine>,
}

#[derive(Clone)]
pub struct OutputPipelineConfig {
    pub path: PathBuf,
}

impl PipelineConfig {
    pub fn from_settings(settings: &EffectiveSettings, input: &Path) -> Result<Self, DecoderError> {
        let engine = build_ocr_engine(settings);
        let output_path = settings
            .output
            .path
            .clone()
            .unwrap_or_else(|| default_output_path(input));
        Ok(Self {
            detection: settings.detection.clone(),
            ocr: OcrPipelineConfig { engine },
            output: OutputPipelineConfig { path: output_path },
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PipelineProgress {
    pub samples_seen: u64,
    pub latest_frame_index: u64,
    pub total_frames: Option<u64>,
    pub fps: f64,
    pub det_ms: f64,
    pub seg_ms: f64,
    pub ocr_ms: f64,
    pub cues: u64,
    pub merged: u64,
    pub ocr_empty: u64,
    pub progress: f64,
    pub completed: bool,
}

#[derive(Clone, Debug)]
pub struct PipelineUpdate {
    pub progress: PipelineProgress,
    pub updates: Vec<SubtitleUpdate>,
}

pub type PipelineResult = AveragerResult;

#[derive(Debug)]
pub enum PipelineError {
    Ocr(OcrStageError),
}

pub struct PipelineOutputs {
    pub stream: Pin<Box<dyn Stream<Item = PipelineResult> + Send>>,
    pub total_frames: Option<u64>,
    pub handle: PipelineHandle,
}

#[derive(Clone)]
pub struct PipelineHandle {
    pause_tx: tokio::sync::watch::Sender<bool>,
}

impl PipelineHandle {
    pub fn pause_sender(&self) -> tokio::sync::watch::Sender<bool> {
        self.pause_tx.clone()
    }

    pub fn set_paused(&self, paused: bool) {
        let _ = self.pause_tx.send(paused);
    }
}

pub fn build_pipeline(
    provider: DynDecoderProvider,
    pipeline: &PipelineConfig,
) -> Result<PipelineOutputs, DecoderError> {
    let initial_total_frames = provider.metadata().total_frames;
    let (_, initial_stream) = provider.open()?;

    let (pause_tx, pause_rx) = tokio::sync::watch::channel(false);

    let paused_stream = StreamBundle::new(
        Box::pin(PauseStream::new(initial_stream, pause_rx.clone())),
        initial_total_frames,
    );

    let sorted = FrameSorter::new().attach(paused_stream);
    let sampled = FrameSampler::new(pipeline.detection.samples_per_second).attach(sorted);

    let detector_stage = Detector::new(&pipeline.detection).map_err(detection_error_to_frame)?;

    let detected = detector_stage.attach(sampled);
    let determined = RegionDeterminer::new().attach(detected);
    let tracked = RegionLifecycleTracker::new(&pipeline.detection).attach(determined);
    let ocred = SubtitleOcr::new(Arc::clone(&pipeline.ocr.engine)).attach(tracked);
    let merged: StreamBundle<MergeResult> = Merge::with_default_window().attach(ocred);
    let averaged: StreamBundle<AveragerResult> = Averager::new().attach(merged);

    Ok(PipelineOutputs {
        stream: averaged.stream,
        total_frames: averaged.total_frames,
        handle: PipelineHandle { pause_tx },
    })
}

struct PauseStream<S> {
    inner: S,
    pause_updates: WatchStream<bool>,
    paused: bool,
}

impl<S> PauseStream<S> {
    fn new(inner: S, pause: tokio::sync::watch::Receiver<bool>) -> Self {
        let paused = *pause.borrow();
        Self {
            inner,
            paused,
            pause_updates: WatchStream::new(pause),
        }
    }
}

impl<S> Stream for PauseStream<S>
where
    S: Stream + Unpin + Send,
{
    type Item = <S as Stream>::Item;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            // Drain any immediately available pause updates.
            while let std::task::Poll::Ready(Some(paused)) =
                Pin::new(&mut this.pause_updates).poll_next(cx)
            {
                this.paused = paused;
            }

            if this.paused {
                // Wait for the next pause update to flip the flag.
                match Pin::new(&mut this.pause_updates).poll_next(cx) {
                    std::task::Poll::Ready(Some(paused)) => {
                        this.paused = paused;
                        continue;
                    }
                    std::task::Poll::Ready(None) => return std::task::Poll::Ready(None),
                    std::task::Poll::Pending => return std::task::Poll::Pending,
                }
            }

            // Not paused; drive the inner stream.
            match Pin::new(&mut this.inner).poll_next(cx) {
                std::task::Poll::Ready(item) => return std::task::Poll::Ready(item),
                std::task::Poll::Pending => {
                    // Allow pause updates to register before parking.
                    if let std::task::Poll::Ready(Some(paused)) =
                        Pin::new(&mut this.pause_updates).poll_next(cx)
                    {
                        this.paused = paused;
                        continue;
                    }
                    return std::task::Poll::Pending;
                }
            }
        }
    }
}

fn detection_error_to_frame(err: SubtitleDetectionError) -> DecoderError {
    DecoderError::configuration(format!("subtitle detection error: {err}"))
}

pub fn pipeline_error_to_frame(err: PipelineError) -> DecoderError {
    match err {
        PipelineError::Ocr(ocr_err) => match ocr_err {
            OcrStageError::Lifecycle(lifecycle_err) => match lifecycle_err {
                RegionLifecycleError::Determiner(det_err) => match det_err {
                    RegionDeterminerError::Detector(detector_err) => match detector_err {
                        detector::DetectorError::Sampler(sampler_err) => sampler_err,
                        detector::DetectorError::Detection(det_err) => {
                            detection_error_to_frame(det_err)
                        }
                    },
                },
            },
            OcrStageError::Engine(ocr_err) => {
                DecoderError::configuration(format!("ocr error: {ocr_err}"))
            }
        },
    }
}

fn build_ocr_engine(settings: &EffectiveSettings) -> Arc<dyn OcrEngine> {
    if let Some(backend) = settings
        .ocr
        .backend
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
    {
        if backend == "auto" {
            return build_ocr_engine_auto();
        }
        if let Some(engine) = build_ocr_engine_requested(&backend) {
            return engine;
        }
        eprintln!("ocr backend '{backend}' unavailable, falling back to auto");
    }
    build_ocr_engine_auto()
}

fn build_ocr_engine_requested(backend: &str) -> Option<Arc<dyn OcrEngine>> {
    match backend {
        "noop" => Some(Arc::new(NoopOcrEngine)),
        "vision" => {
            #[cfg(all(feature = "ocr-vision", target_os = "macos"))]
            {
                return VisionOcrEngine::new()
                    .map(|engine| Arc::new(engine) as Arc<dyn OcrEngine>)
                    .map_err(|err| {
                        eprintln!("vision OCR engine failed to initialize: {err}");
                        err
                    })
                    .ok();
            }
            #[allow(unreachable_code)]
            None
        }
        "ort" => {
            #[cfg(feature = "ocr-ort")]
            {
                return build_ort_engine()
                    .map_err(|err| {
                        eprintln!("ort OCR engine failed to initialize: {err}");
                        err
                    })
                    .ok();
            }
            #[allow(unreachable_code)]
            None
        }
        _ => None,
    }
}

fn build_ocr_engine_auto() -> Arc<dyn OcrEngine> {
    #[cfg(all(feature = "ocr-vision", target_os = "macos"))]
    {
        match VisionOcrEngine::new() {
            Ok(engine) => return Arc::new(engine),
            Err(err) => {
                eprintln!("vision OCR engine failed to initialize: {err}");
            }
        }
    }
    #[cfg(feature = "ocr-ort")]
    {
        match build_ort_engine() {
            Ok(engine) => return engine,
            Err(err) => {
                eprintln!("ort OCR engine failed to initialize: {err}");
            }
        }
    }
    Arc::new(NoopOcrEngine)
}

#[cfg(feature = "ocr-ort")]
fn build_ort_engine() -> Result<Arc<dyn OcrEngine>, OcrError> {
    let paths = model::ort_model_paths()
        .map_err(|err| OcrError::backend(format!("failed to resolve ORT model paths: {err}")))?;
    let config = OrtOcrConfig {
        model_path: paths.model_path().to_path_buf(),
        dictionary_path: paths.dictionary_path().to_path_buf(),
        ..OrtOcrConfig::default()
    };
    OrtOcrEngine::with_config(config).map(|engine| Arc::new(engine) as Arc<dyn OcrEngine>)
}

fn default_output_path(input: &Path) -> PathBuf {
    let mut path = input.to_path_buf();
    path.set_extension("srt");
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use futures_util::stream;
    use futures_util::task::noop_waker_ref;
    use std::num::NonZero;
    use std::path::Path;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use subtitle_fast_ocr::OcrError;
    use subtitle_fast_types::RoiConfig;
    use subtitle_fast_validator::subtitle_detection::SubtitleDetectorKind;

    #[test]
    fn default_output_path_changes_extension_to_srt() {
        let path = default_output_path(Path::new("/tmp/demo/video.mp4"));
        assert_eq!(path, PathBuf::from("/tmp/demo/video.srt"));
    }

    #[test]
    fn pipeline_error_mapping_is_configuration_error() {
        let error = pipeline_error_to_frame(PipelineError::Ocr(OcrStageError::Engine(
            OcrError::backend("boom"),
        )));
        assert!(
            error
                .to_string()
                .contains("configuration error: ocr error: backend error: boom")
        );
    }

    #[test]
    fn pipeline_config_uses_default_output_and_noop_engine_when_requested() {
        let settings = EffectiveSettings {
            detection: DetectionSettings {
                samples_per_second: 5,
                target: 220,
                delta: 10,
                detector: subtitle_fast_validator::subtitle_detection::SubtitleDetectorKind::ProjectionBand,
                comparator: None,
                roi: None,
            },
            decoder: crate::settings::DecoderSettings::default(),
            ocr: crate::settings::OcrSettings {
                backend: Some("noop".to_string()),
            },
            output: crate::settings::OutputSettings::default(),
        };

        let config = PipelineConfig::from_settings(&settings, Path::new("movie.mkv"))
            .expect("pipeline config");
        assert_eq!(config.output.path, PathBuf::from("movie.srt"));
        assert_eq!(config.ocr.engine.name(), "noop");

        let explicit = EffectiveSettings {
            output: crate::settings::OutputSettings {
                path: Some(PathBuf::from("custom-output.srt")),
            },
            ..settings
        };
        let explicit_config = PipelineConfig::from_settings(&explicit, Path::new("movie.mkv"))
            .expect("pipeline config with explicit output");
        assert_eq!(
            explicit_config.output.path,
            PathBuf::from("custom-output.srt")
        );
    }

    #[test]
    fn build_ocr_engine_falls_back_to_noop_for_unknown_backend() {
        let settings = EffectiveSettings {
            detection: DetectionSettings {
                samples_per_second: 5,
                target: 220,
                delta: 10,
                detector: subtitle_fast_validator::subtitle_detection::SubtitleDetectorKind::ProjectionBand,
                comparator: None,
                roi: None,
            },
            decoder: crate::settings::DecoderSettings::default(),
            ocr: crate::settings::OcrSettings {
                backend: Some("unknown-backend".to_string()),
            },
            output: crate::settings::OutputSettings::default(),
        };

        let engine = build_ocr_engine(&settings);
        let auto = build_ocr_engine_auto();
        assert_eq!(engine.name(), auto.name());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pause_stream_respects_pause_state_transitions() {
        let (tx2, rx2) = tokio::sync::watch::channel(true);
        let source2 = stream::iter(vec![10u8, 20]);
        let mut paused2 = PauseStream::new(source2, rx2);

        let waker = noop_waker_ref();
        let mut context = Context::from_waker(waker);
        assert!(matches!(
            Pin::new(&mut paused2).poll_next(&mut context),
            Poll::Pending
        ));

        tx2.send(false).expect("resume");
        let next = futures_util::StreamExt::next(&mut paused2).await;
        assert_eq!(next, Some(10));
    }

    #[test]
    fn pipeline_handle_clones_pause_sender() {
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        let handle = PipelineHandle { pause_tx: tx };
        handle.set_paused(true);
        assert_eq!(*rx.borrow_and_update(), true);

        let sender = handle.pause_sender();
        sender.send(false).expect("send pause state");
        assert_eq!(*rx.borrow_and_update(), false);
    }

    #[test]
    fn stream_bundle_new_preserves_total_frames() {
        let stream = Box::pin(stream::iter(vec![1u8, 2u8]));
        let bundle = StreamBundle::new(stream, Some(88));
        assert_eq!(bundle.total_frames, Some(88));
    }

    #[test]
    fn pipeline_progress_defaults_to_zero_values() {
        let progress = PipelineProgress::default();
        assert_eq!(progress.samples_seen, 0);
        assert_eq!(progress.latest_frame_index, 0);
        assert_eq!(progress.total_frames, None);
        assert!(!progress.completed);
        assert!((progress.progress - 0.0).abs() < f64::EPSILON);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn build_pipeline_with_mock_provider_emits_completion_update() {
        let decoder_config = subtitle_fast_decoder::Configuration {
            backend: subtitle_fast_decoder::Backend::Mock,
            input: None,
            channel_capacity: NonZero::new(8),
            output_format: subtitle_fast_decoder::OutputFormat::Nv12,
            start_frame: Some(0),
        };
        let provider = Box::new(
            <subtitle_fast_decoder::backends::mock::MockProvider as subtitle_fast_decoder::DecoderProvider>::new(&decoder_config)
                .expect("mock provider"),
        ) as subtitle_fast_decoder::DynDecoderProvider;

        let settings = EffectiveSettings {
            detection: DetectionSettings {
                samples_per_second: 12,
                target: 230,
                delta: 12,
                detector: SubtitleDetectorKind::ProjectionBand,
                comparator: None,
                roi: Some(RoiConfig {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                }),
            },
            decoder: crate::settings::DecoderSettings::default(),
            ocr: crate::settings::OcrSettings {
                backend: Some("noop".to_string()),
            },
            output: crate::settings::OutputSettings::default(),
        };
        let pipeline_config =
            PipelineConfig::from_settings(&settings, Path::new("demo.mp4")).expect("config");
        let outputs = build_pipeline(provider, &pipeline_config).expect("build pipeline");
        assert_eq!(outputs.total_frames, Some(120));

        outputs.handle.set_paused(true);
        outputs.handle.set_paused(false);
        outputs
            .handle
            .pause_sender()
            .send(false)
            .expect("send pause");

        let mut stream = outputs.stream;
        let mut saw_any = false;
        let mut saw_completed = false;
        while let Some(update) = stream.next().await {
            let update = update.expect("pipeline item");
            saw_any = true;
            if update.progress.completed {
                saw_completed = true;
                break;
            }
        }

        assert!(saw_any);
        assert!(saw_completed);
    }
}
