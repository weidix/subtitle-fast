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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{Backend, Configuration};
    use crate::pipeline::PreprocessSettings;

    #[test]
    fn backend_parse_is_trimmed_case_insensitive_and_rejects_unknown() {
        assert_eq!(
            Backend::from_str("  BITSET-COVER\n").expect("bitset-cover should parse"),
            Backend::BitsetCover
        );
        assert_eq!(
            Backend::from_str(" sparse-chamfer ").expect("sparse-chamfer should parse"),
            Backend::SparseChamfer
        );

        let err = Backend::from_str("unknown-backend").expect_err("unknown should fail");
        assert_eq!(err.0, "unknown-backend");
        assert_eq!(err.to_string(), "unknown comparator 'unknown-backend'");
    }

    #[test]
    fn configuration_factory_builds_expected_comparator_types() {
        let preprocess = PreprocessSettings {
            target: 200,
            delta: 20,
        };

        let bitset = Configuration {
            backend: Backend::BitsetCover,
            preprocess,
        }
        .create_comparator();
        assert_eq!(bitset.name(), "bitset-cover");

        let sparse = Configuration {
            backend: Backend::SparseChamfer,
            preprocess,
        }
        .create_comparator();
        assert_eq!(sparse.name(), "sparse-chamfer");
    }
}
