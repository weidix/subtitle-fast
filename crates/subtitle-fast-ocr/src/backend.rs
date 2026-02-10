use std::fmt;
use std::str::FromStr;

use crate::{NoopOcrEngine, OcrEngine, OcrError};

#[cfg(feature = "engine-ort")]
use crate::OrtOcrEngine;
#[cfg(all(feature = "engine-vision", target_os = "macos"))]
use crate::VisionOcrEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Auto,
    Noop,
    #[cfg(feature = "engine-ort")]
    Ort,
    #[cfg(all(feature = "engine-vision", target_os = "macos"))]
    Vision,
}

#[derive(Debug, Clone)]
pub struct Configuration {
    pub backend: Backend,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            backend: default_backend(),
        }
    }
}

impl Configuration {
    pub fn available_backends() -> Vec<Backend> {
        compiled_backends()
    }

    pub fn create_engine(&self) -> Result<Box<dyn OcrEngine>, OcrError> {
        match self.backend {
            Backend::Auto => build_auto_engine(),
            Backend::Noop => Ok(Box::new(NoopOcrEngine)),
            #[cfg(feature = "engine-ort")]
            Backend::Ort => build_ort_engine(),
            #[cfg(all(feature = "engine-vision", target_os = "macos"))]
            Backend::Vision => build_vision_engine(),
        }
    }
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Auto => "auto",
            Backend::Noop => "noop",
            #[cfg(feature = "engine-ort")]
            Backend::Ort => "ort",
            #[cfg(all(feature = "engine-vision", target_os = "macos"))]
            Backend::Vision => "vision",
        }
    }

    pub fn available() -> Vec<Backend> {
        Configuration::available_backends()
    }

    pub fn create_engine(self) -> Result<Box<dyn OcrEngine>, OcrError> {
        Configuration { backend: self }.create_engine()
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Backend {
    type Err = OcrError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Backend::Auto),
            "noop" => Ok(Backend::Noop),
            #[cfg(feature = "engine-ort")]
            "ort" => Ok(Backend::Ort),
            #[cfg(all(feature = "engine-vision", target_os = "macos"))]
            "vision" => Ok(Backend::Vision),
            other => Err(OcrError::backend(format!("unknown OCR backend '{other}'"))),
        }
    }
}

fn compiled_backends() -> Vec<Backend> {
    vec![
        #[cfg(feature = "engine-ort")]
        Backend::Ort,
        #[cfg(all(feature = "engine-vision", target_os = "macos"))]
        Backend::Vision,
        Backend::Noop,
    ]
}

fn default_backend() -> Backend {
    compiled_backends()
        .into_iter()
        .next()
        .unwrap_or(Backend::Noop)
}

fn build_auto_engine() -> Result<Box<dyn OcrEngine>, OcrError> {
    if let Ok(engine) = build_ort_engine() {
        return Ok(engine);
    }
    if let Ok(engine) = build_vision_engine() {
        return Ok(engine);
    }
    Ok(Box::new(NoopOcrEngine))
}

fn build_ort_engine() -> Result<Box<dyn OcrEngine>, OcrError> {
    #[cfg(feature = "engine-ort")]
    {
        return Ok(Box::new(OrtOcrEngine::new()?));
    }
    #[allow(unreachable_code)]
    Err(OcrError::backend(
        "ort backend not available (feature engine-ort disabled)",
    ))
}

fn build_vision_engine() -> Result<Box<dyn OcrEngine>, OcrError> {
    #[cfg(all(feature = "engine-vision", target_os = "macos"))]
    {
        return Ok(Box::new(VisionOcrEngine::new()?));
    }
    #[allow(unreachable_code)]
    Err(OcrError::backend(
        "vision backend not available on this target",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn backend_parse_is_case_insensitive_and_trimmed() {
        assert_eq!(Backend::from_str(" auto ").expect("auto"), Backend::Auto);
        assert_eq!(Backend::from_str("NOOP").expect("noop"), Backend::Noop);
    }

    #[test]
    fn backend_parse_rejects_unknown_value() {
        let err = Backend::from_str("something-else").expect_err("must fail");
        assert!(err.to_string().contains("unknown OCR backend"));
    }

    #[test]
    fn backend_display_matches_wire_values() {
        assert_eq!(Backend::Auto.to_string(), "auto");
        assert_eq!(Backend::Noop.to_string(), "noop");
    }

    #[test]
    fn available_backends_always_include_noop() {
        let backends = Configuration::available_backends();
        assert!(backends.contains(&Backend::Noop));
    }

    #[test]
    fn noop_engine_can_always_be_constructed() {
        let engine = Backend::Noop.create_engine().expect("noop engine");
        assert_eq!(engine.name(), "noop");
        engine.warm_up().expect("noop warmup");
    }
}
