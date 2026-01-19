use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use crate::comparators::{BitsetCoverComparator, SparseChamferComparator, SubtitleComparator};
use crate::pipeline::PreprocessSettings;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Backend {
    BitsetCover,
    SparseChamfer,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::BitsetCover => "bitset-cover",
            Backend::SparseChamfer => "sparse-chamfer",
        }
    }

    pub fn available() -> Vec<Backend> {
        Configuration::available_backends()
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug)]
pub struct BackendParseError(pub String);

impl fmt::Display for BackendParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown comparator '{}'", self.0)
    }
}

impl std::error::Error for BackendParseError {}

impl FromStr for Backend {
    type Err = BackendParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let lower = s.trim().to_ascii_lowercase();
        match lower.as_str() {
            "bitset-cover" => Ok(Backend::BitsetCover),
            "sparse-chamfer" => Ok(Backend::SparseChamfer),
            _ => Err(BackendParseError(lower)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Configuration {
    pub backend: Backend,
    pub preprocess: PreprocessSettings,
}

impl Configuration {
    pub fn available_backends() -> Vec<Backend> {
        vec![Backend::BitsetCover, Backend::SparseChamfer]
    }

    pub fn create_comparator(&self) -> Arc<dyn SubtitleComparator> {
        match self.backend {
            Backend::BitsetCover => Arc::new(BitsetCoverComparator::new(self.preprocess)),
            Backend::SparseChamfer => Arc::new(SparseChamferComparator::new(self.preprocess)),
        }
    }
}

pub type ComparatorKind = Backend;
pub type ComparatorKindParseError = BackendParseError;
