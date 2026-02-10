use std::env;
use std::fmt;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::str::FromStr;

#[cfg(feature = "backend-ffmpeg")]
use std::sync::OnceLock;

use crate::core::{DecoderError, DecoderProvider, DecoderResult, DynDecoderProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Mock,
    #[cfg(feature = "backend-ffmpeg")]
    FFmpeg,
    #[cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]
    VideoToolbox,
    #[cfg(all(feature = "backend-dxva", target_os = "windows"))]
    Dxva,
    #[cfg(all(feature = "backend-mft", target_os = "windows"))]
    Mft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Nv12,
    CVPixelBuffer,
}

impl OutputFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFormat::Nv12 => "nv12",
            OutputFormat::CVPixelBuffer => "cvpixelbuffer",
        }
    }
}

impl FromStr for Backend {
    type Err = DecoderError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "mock" => Ok(Backend::Mock),
            #[cfg(feature = "backend-ffmpeg")]
            "ffmpeg" => Ok(Backend::FFmpeg),
            #[cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]
            "videotoolbox" => Ok(Backend::VideoToolbox),
            #[cfg(all(feature = "backend-dxva", target_os = "windows"))]
            "dxva" => Ok(Backend::Dxva),
            #[cfg(all(feature = "backend-mft", target_os = "windows"))]
            "mft" => Ok(Backend::Mft),
            other => Err(DecoderError::configuration(format!(
                "unknown backend '{other}'"
            ))),
        }
    }
}

impl Backend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::Mock => "mock",
            #[cfg(feature = "backend-ffmpeg")]
            Backend::FFmpeg => "ffmpeg",
            #[cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]
            Backend::VideoToolbox => "videotoolbox",
            #[cfg(all(feature = "backend-dxva", target_os = "windows"))]
            Backend::Dxva => "dxva",
            #[cfg(all(feature = "backend-mft", target_os = "windows"))]
            Backend::Mft => "mft",
            #[allow(unreachable_patterns)]
            _ => "unsupported",
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn compiled_backends() -> Vec<Backend> {
    let mut backends = Vec::new();
    if github_ci_active() {
        backends.push(Backend::Mock);
    }
    append_platform_backends(&mut backends);
    backends
}

#[cfg(all(
    target_os = "macos",
    any(feature = "backend-videotoolbox", feature = "backend-ffmpeg")
))]
fn append_platform_backends(backends: &mut Vec<Backend>) {
    #[cfg(feature = "backend-videotoolbox")]
    {
        backends.push(Backend::VideoToolbox);
    }
    #[cfg(feature = "backend-ffmpeg")]
    {
        if ffmpeg_runtime_available() {
            backends.push(Backend::FFmpeg);
        }
    }
}

#[cfg(all(
    target_os = "macos",
    not(any(feature = "backend-videotoolbox", feature = "backend-ffmpeg"))
))]
fn append_platform_backends(_backends: &mut [Backend]) {}

#[cfg(all(
    not(target_os = "macos"),
    any(
        feature = "backend-mft",
        feature = "backend-dxva",
        feature = "backend-ffmpeg"
    )
))]
fn append_platform_backends(backends: &mut Vec<Backend>) {
    #[cfg(all(feature = "backend-mft", target_os = "windows"))]
    {
        backends.push(Backend::Mft);
    }
    #[cfg(all(feature = "backend-dxva", target_os = "windows"))]
    {
        backends.push(Backend::Dxva);
    }
    #[cfg(feature = "backend-ffmpeg")]
    {
        if ffmpeg_runtime_available() {
            backends.push(Backend::FFmpeg);
        }
    }
}

#[cfg(all(
    not(target_os = "macos"),
    not(any(
        feature = "backend-mft",
        feature = "backend-dxva",
        feature = "backend-ffmpeg"
    ))
))]
fn append_platform_backends(_backends: &mut [Backend]) {}

#[cfg(feature = "backend-ffmpeg")]
fn ffmpeg_runtime_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| match ffmpeg_next::init() {
        Ok(()) => true,
        Err(err) => {
            eprintln!("ffmpeg backend disabled: failed to initialize libraries ({err})");
            false
        }
    })
}

#[derive(Debug, Clone)]
pub struct Configuration {
    pub backend: Backend,
    pub input: Option<PathBuf>,
    pub channel_capacity: Option<NonZeroUsize>,
    pub output_format: OutputFormat,
    pub start_frame: Option<u64>,
}

impl Default for Configuration {
    fn default() -> Self {
        let backend = compiled_backends()
            .into_iter()
            .next()
            .unwrap_or_else(default_backend);
        Self {
            backend,
            input: None,
            channel_capacity: None,
            output_format: OutputFormat::Nv12,
            start_frame: None,
        }
    }
}

impl Configuration {
    pub fn from_env() -> DecoderResult<Self> {
        let mut config = Configuration::default();
        if let Ok(backend) = env::var("SUBFAST_BACKEND") {
            config.backend = Backend::from_str(&backend)?;
        }
        if let Ok(path) = env::var("SUBFAST_INPUT") {
            config.input = Some(PathBuf::from(path));
        }
        if let Ok(capacity) = env::var("SUBFAST_CHANNEL_CAPACITY") {
            let parsed: usize = capacity.parse().map_err(|_| {
                DecoderError::configuration(format!(
                    "failed to parse SUBFAST_CHANNEL_CAPACITY='{capacity}' as a positive integer"
                ))
            })?;
            let Some(value) = NonZeroUsize::new(parsed) else {
                return Err(DecoderError::configuration(
                    "SUBFAST_CHANNEL_CAPACITY must be greater than zero",
                ));
            };
            config.channel_capacity = Some(value);
        }
        if let Ok(start_frame) = env::var("SUBFAST_START_FRAME") {
            let parsed: u64 = start_frame.parse().map_err(|_| {
                DecoderError::configuration(format!(
                    "failed to parse SUBFAST_START_FRAME='{start_frame}' as a non-negative integer"
                ))
            })?;
            config.start_frame = Some(parsed);
        }
        Ok(config)
    }

    pub fn available_backends() -> Vec<Backend> {
        compiled_backends()
    }

    pub fn create_provider(&self) -> DecoderResult<DynDecoderProvider> {
        self.validate_output_format()?;

        match self.backend {
            Backend::Mock => {
                if !github_ci_active() {
                    Err(DecoderError::unsupported("mock"))
                } else {
                    Ok(Box::new(crate::backends::mock::MockProvider::new(self)?))
                }
            }
            #[cfg(feature = "backend-ffmpeg")]
            Backend::FFmpeg => Ok(Box::new(crate::backends::ffmpeg::FFmpegProvider::new(
                self,
            )?)),
            #[cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]
            Backend::VideoToolbox => Ok(Box::new(
                crate::backends::videotoolbox::VideoToolboxProvider::new(self)?,
            )),
            #[cfg(all(feature = "backend-dxva", target_os = "windows"))]
            Backend::Dxva => Ok(Box::new(crate::backends::dxva::DxvaProvider::new(self)?)),
            #[cfg(all(feature = "backend-mft", target_os = "windows"))]
            Backend::Mft => Ok(Box::new(crate::backends::mft::MftProvider::new(self)?)),
            #[allow(unreachable_patterns)]
            other => Err(DecoderError::unsupported(other.as_str())),
        }
    }
}

impl Configuration {
    fn validate_output_format(&self) -> DecoderResult<()> {
        match self.output_format {
            OutputFormat::Nv12 => Ok(()),
            OutputFormat::CVPixelBuffer => {
                #[cfg(all(feature = "backend-videotoolbox", target_os = "macos"))]
                {
                    if self.backend == Backend::VideoToolbox {
                        return Ok(());
                    }
                }

                Err(DecoderError::configuration(format!(
                    "output format '{}' is only supported by videotoolbox backend (selected: {})",
                    self.output_format.as_str(),
                    self.backend.as_str()
                )))
            }
        }
    }
}

fn default_backend() -> Backend {
    if github_ci_active() {
        return Backend::Mock;
    }
    #[cfg(feature = "backend-ffmpeg")]
    return Backend::FFmpeg;

    #[allow(unreachable_code)]
    Backend::Mock
}

fn github_ci_active() -> bool {
    env::var("GITHUB_ACTIONS")
        .map(|value| !value.is_empty() && value != "false")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_mutex() -> &'static Mutex<()> {
        static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
        ENV_MUTEX.get_or_init(|| Mutex::new(()))
    }

    fn set_env_var(key: &str, value: &str) {
        unsafe { std::env::set_var(key, value) }
    }

    fn remove_env_var(key: &str) {
        unsafe { std::env::remove_var(key) }
    }

    #[test]
    fn output_format_string_representation_is_stable() {
        assert_eq!(OutputFormat::Nv12.as_str(), "nv12");
        assert_eq!(OutputFormat::CVPixelBuffer.as_str(), "cvpixelbuffer");
    }

    #[test]
    fn backend_parse_accepts_mock_and_rejects_unknown() {
        assert_eq!(Backend::from_str("mock").expect("mock"), Backend::Mock);

        let err = Backend::from_str("unknown").expect_err("must fail");
        assert!(err.to_string().contains("unknown backend"));
    }

    #[test]
    fn duration_and_frame_count_calculation_follow_expected_rules() {
        let metadata = crate::core::VideoMetadata::with_duration_and_fps(
            std::time::Duration::from_secs_f64(2.5),
            30.0,
        );

        assert_eq!(metadata.duration_ms(), Some(2500.0));
        assert_eq!(metadata.calculate_total_frames(), Some(75));
    }

    #[test]
    fn total_frames_field_has_priority_over_estimation() {
        let metadata = crate::core::VideoMetadata {
            duration: Some(std::time::Duration::from_secs(10)),
            fps: Some(10.0),
            width: None,
            height: None,
            total_frames: Some(42),
        };

        assert_eq!(metadata.calculate_total_frames(), Some(42));
    }

    #[test]
    fn github_actions_flag_controls_detection() {
        let _guard = env_mutex().lock().expect("env mutex");
        set_env_var("GITHUB_ACTIONS", "true");
        assert!(github_ci_active());

        set_env_var("GITHUB_ACTIONS", "false");
        assert!(!github_ci_active());

        remove_env_var("GITHUB_ACTIONS");
        assert!(!github_ci_active());
    }

    #[test]
    fn from_env_parses_capacity_and_start_frame() {
        let _guard = env_mutex().lock().expect("env mutex");
        set_env_var("SUBFAST_BACKEND", "mock");
        set_env_var("SUBFAST_CHANNEL_CAPACITY", "6");
        set_env_var("SUBFAST_START_FRAME", "12");

        let config = Configuration::from_env().expect("config");
        assert_eq!(config.backend, Backend::Mock);
        assert_eq!(config.channel_capacity.map(|v| v.get()), Some(6));
        assert_eq!(config.start_frame, Some(12));

        remove_env_var("SUBFAST_BACKEND");
        remove_env_var("SUBFAST_CHANNEL_CAPACITY");
        remove_env_var("SUBFAST_START_FRAME");
    }

    #[test]
    fn from_env_rejects_invalid_capacity() {
        let _guard = env_mutex().lock().expect("env mutex");
        set_env_var("SUBFAST_CHANNEL_CAPACITY", "0");
        let err = Configuration::from_env().expect_err("must fail");
        assert!(
            err.to_string()
                .contains("SUBFAST_CHANNEL_CAPACITY must be greater than zero")
        );
        remove_env_var("SUBFAST_CHANNEL_CAPACITY");
    }

    #[test]
    fn create_provider_rejects_mock_outside_github_actions() {
        let _guard = env_mutex().lock().expect("env mutex");
        remove_env_var("GITHUB_ACTIONS");
        let config = Configuration {
            backend: Backend::Mock,
            ..Configuration::default()
        };
        let err = config.create_provider().err().expect("must reject mock");
        assert!(err.to_string().contains("not supported"));
    }

    #[test]
    fn create_provider_accepts_mock_in_github_actions_mode() {
        let _guard = env_mutex().lock().expect("env mutex");
        set_env_var("GITHUB_ACTIONS", "true");
        let config = Configuration {
            backend: Backend::Mock,
            ..Configuration::default()
        };
        assert!(config.create_provider().is_ok());
        remove_env_var("GITHUB_ACTIONS");
    }
}
