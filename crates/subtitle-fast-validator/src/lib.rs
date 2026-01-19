pub mod subtitle_detection;

mod config;
mod detection;
mod validator;

pub use config::{FrameValidatorConfig, SubtitleDetectionOptions};
pub use subtitle_detection::{Configuration, RoiConfig};
pub use validator::FrameValidator;
