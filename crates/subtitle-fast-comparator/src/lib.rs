//! Comparator crate entry point with flat, easy-to-import modules.

pub mod comparators;
pub mod config;
pub mod pipeline;

pub use comparators::{BitsetCoverComparator, SparseChamferComparator, SubtitleComparator};
pub use config::{Backend, ComparatorKind, ComparatorKindParseError, Configuration};
pub use pipeline::{ComparisonReport, FeatureBlob, PreprocessSettings, ReportMetric};

#[cfg(test)]
mod tests;
