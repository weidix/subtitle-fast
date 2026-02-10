use crate::error::OcrError;
use crate::request::OcrRequest;
use crate::response::OcrResponse;

/// Common interface for all OCR engines.
pub trait OcrEngine: Send + Sync {
    fn name(&self) -> &'static str;

    fn warm_up(&self) -> Result<(), OcrError> {
        Ok(())
    }

    fn recognize(&self, request: &OcrRequest<'_>) -> Result<OcrResponse, OcrError>;
}

/// Placeholder OCR engine used while a real backend is not wired.
#[derive(Debug, Default)]
pub struct NoopOcrEngine;

impl OcrEngine for NoopOcrEngine {
    fn name(&self) -> &'static str {
        "noop"
    }

    fn recognize(&self, _: &OcrRequest<'_>) -> Result<OcrResponse, OcrError> {
        Ok(OcrResponse::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LumaPlane, OcrRegion};

    #[test]
    fn noop_engine_returns_empty_response() {
        let engine = NoopOcrEngine;
        let bytes = vec![10u8; 16];
        let plane = LumaPlane::from_parts(4, 4, 4, &bytes).expect("plane");
        let regions = [OcrRegion::new(0.0, 0.0, 2.0, 2.0)];
        let request = OcrRequest::new(plane, &regions);

        assert_eq!(engine.name(), "noop");
        engine.warm_up().expect("warmup");
        let response = engine.recognize(&request).expect("recognize");
        assert!(response.texts.is_empty());
    }
}
