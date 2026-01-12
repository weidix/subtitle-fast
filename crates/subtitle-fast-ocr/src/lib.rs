mod backends;
mod engine;
mod error;
mod plane;
mod region;
mod request;
mod response;

#[cfg(all(feature = "engine-vision", target_os = "macos"))]
pub use backends::vision::{VisionOcrConfig, VisionOcrEngine};
#[cfg(feature = "engine-ort")]
pub use backends::ort::{OrtOcrConfig, OrtOcrEngine};
pub use engine::{NoopOcrEngine, OcrEngine};
pub use error::OcrError;
pub use plane::LumaPlane;
pub use region::OcrRegion;
pub use request::OcrRequest;
pub use response::{OcrResponse, OcrText};
