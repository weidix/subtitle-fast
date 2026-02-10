//! Shared domain models for the subtitle-fast workspace.
//!
//! This crate centralizes lightweight data structures used across decoder,
//! validator, comparator, OCR, and CLI crates. Keep it backend-agnostic and
//! avoid platform-specific dependencies so all crates can depend on it without
//! pulling native SDKs or heavy features.

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

pub type DecoderResult<T> = Result<T, DecoderError>;

#[derive(Clone)]
pub struct VideoFrame {
    width: u32,
    height: u32,
    serial: u64,
    index: Option<u64>,
    pts: Option<Duration>,
    dts: Option<Duration>,
    buffer: FrameBuffer,
}

#[derive(Clone)]
pub enum FrameBuffer {
    Nv12(Nv12Buffer),
    Native(NativeBuffer),
}

#[derive(Clone)]
pub struct Nv12Buffer {
    y_stride: usize,
    uv_stride: usize,
    y_plane: Arc<[u8]>,
    uv_plane: Arc<[u8]>,
}

#[derive(Clone)]
pub struct NativeBuffer {
    backend: &'static str,
    pixel_format: u32,
    handle: Arc<NativeHandle>,
}

struct NativeHandle {
    handle: NonNull<c_void>,
    release: unsafe extern "C" fn(*mut c_void),
}

// Native handles are ref-counted by the backend, and release callbacks are thread-safe.
unsafe impl Send for NativeHandle {}
unsafe impl Sync for NativeHandle {}

impl Drop for NativeHandle {
    fn drop(&mut self) {
        unsafe { (self.release)(self.handle.as_ptr()) };
    }
}

impl NativeBuffer {
    pub fn backend(&self) -> &'static str {
        self.backend
    }

    pub fn pixel_format(&self) -> u32 {
        self.pixel_format
    }

    pub fn handle(&self) -> *mut c_void {
        self.handle.handle.as_ptr()
    }
}

impl Nv12Buffer {
    pub fn y_stride(&self) -> usize {
        self.y_stride
    }

    pub fn uv_stride(&self) -> usize {
        self.uv_stride
    }

    pub fn y_plane(&self) -> &[u8] {
        &self.y_plane
    }

    pub fn uv_plane(&self) -> &[u8] {
        &self.uv_plane
    }
}

impl fmt::Debug for VideoFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.buffer {
            FrameBuffer::Nv12(buffer) => f
                .debug_struct("VideoFrame")
                .field("width", &self.width)
                .field("height", &self.height)
                .field("format", &"nv12")
                .field("y_stride", &buffer.y_stride)
                .field("uv_stride", &buffer.uv_stride)
                .field("y_bytes", &buffer.y_plane.len())
                .field("uv_bytes", &buffer.uv_plane.len())
                .field("pts", &self.pts)
                .field("dts", &self.dts)
                .field("serial", &self.serial)
                .field("index", &self.index)
                .finish(),
            FrameBuffer::Native(buffer) => f
                .debug_struct("VideoFrame")
                .field("width", &self.width)
                .field("height", &self.height)
                .field("format", &"native-handle")
                .field("backend", &buffer.backend)
                .field("pixel_format", &buffer.pixel_format)
                .field("handle", &buffer.handle())
                .field("pts", &self.pts)
                .field("dts", &self.dts)
                .field("serial", &self.serial)
                .field("index", &self.index)
                .finish(),
        }
    }
}

impl VideoFrame {
    #[allow(clippy::too_many_arguments)]
    pub fn from_nv12_owned(
        width: u32,
        height: u32,
        y_stride: usize,
        uv_stride: usize,
        pts: Option<Duration>,
        dts: Option<Duration>,
        mut y_plane: Vec<u8>,
        mut uv_plane: Vec<u8>,
    ) -> DecoderResult<Self> {
        let y_required =
            y_stride
                .checked_mul(height as usize)
                .ok_or_else(|| DecoderError::InvalidFrame {
                    reason: "calculated NV12 Y plane length overflowed".into(),
                })?;
        let uv_rows = nv12_uv_rows(height);
        let uv_required =
            uv_stride
                .checked_mul(uv_rows)
                .ok_or_else(|| DecoderError::InvalidFrame {
                    reason: "calculated NV12 UV plane length overflowed".into(),
                })?;

        if y_plane.len() < y_required {
            return Err(DecoderError::InvalidFrame {
                reason: format!(
                    "insufficient NV12 Y plane bytes: got {} expected at least {}",
                    y_plane.len(),
                    y_required
                ),
            });
        }
        if uv_plane.len() < uv_required {
            return Err(DecoderError::InvalidFrame {
                reason: format!(
                    "insufficient NV12 UV plane bytes: got {} expected at least {}",
                    uv_plane.len(),
                    uv_required
                ),
            });
        }

        y_plane.truncate(y_required);
        uv_plane.truncate(uv_required);

        Ok(Self {
            width,
            height,
            pts,
            dts,
            serial: 0,
            index: None,
            buffer: FrameBuffer::Nv12(Nv12Buffer {
                y_stride,
                uv_stride,
                y_plane: Arc::from(y_plane.into_boxed_slice()),
                uv_plane: Arc::from(uv_plane.into_boxed_slice()),
            }),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_native_handle(
        width: u32,
        height: u32,
        pts: Option<Duration>,
        dts: Option<Duration>,
        index: Option<u64>,
        backend: &'static str,
        pixel_format: u32,
        handle: *mut c_void,
        release: unsafe extern "C" fn(*mut c_void),
    ) -> DecoderResult<Self> {
        let handle = NonNull::new(handle).ok_or_else(|| DecoderError::InvalidFrame {
            reason: "native handle is null".into(),
        })?;

        Ok(Self {
            width,
            height,
            pts,
            dts,
            serial: 0,
            index,
            buffer: FrameBuffer::Native(NativeBuffer {
                backend,
                pixel_format,
                handle: Arc::new(NativeHandle { handle, release }),
            }),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pts(&self) -> Option<Duration> {
        self.pts
    }

    pub fn dts(&self) -> Option<Duration> {
        self.dts
    }

    pub fn serial(&self) -> u64 {
        self.serial
    }

    pub fn index(&self) -> Option<u64> {
        self.index
    }

    pub fn buffer(&self) -> &FrameBuffer {
        &self.buffer
    }

    pub fn nv12(&self) -> &Nv12Buffer {
        self.expect_nv12()
    }

    pub fn native(&self) -> Option<&NativeBuffer> {
        match &self.buffer {
            FrameBuffer::Native(buffer) => Some(buffer),
            _ => None,
        }
    }

    pub fn stride(&self) -> usize {
        self.expect_nv12().y_stride
    }

    pub fn y_stride(&self) -> usize {
        self.expect_nv12().y_stride
    }

    pub fn uv_stride(&self) -> usize {
        self.expect_nv12().uv_stride
    }

    pub fn data(&self) -> &[u8] {
        &self.expect_nv12().y_plane
    }

    pub fn y_plane(&self) -> &[u8] {
        &self.expect_nv12().y_plane
    }

    pub fn uv_plane(&self) -> &[u8] {
        &self.expect_nv12().uv_plane
    }

    pub fn with_serial(mut self, serial: u64) -> Self {
        self.serial = serial;
        self
    }

    pub fn set_serial(&mut self, serial: u64) {
        self.serial = serial;
    }

    pub fn with_index(mut self, index: Option<u64>) -> Self {
        self.index = index;
        self
    }

    pub fn set_index(&mut self, index: Option<u64>) {
        self.index = index;
    }

    pub fn with_pts(mut self, pts: Option<Duration>) -> Self {
        self.pts = pts;
        self
    }

    pub fn set_pts(&mut self, pts: Option<Duration>) {
        self.pts = pts;
    }

    pub fn with_dts(mut self, dts: Option<Duration>) -> Self {
        self.dts = dts;
        self
    }

    pub fn set_dts(&mut self, dts: Option<Duration>) {
        self.dts = dts;
    }

    fn expect_nv12(&self) -> &Nv12Buffer {
        match &self.buffer {
            FrameBuffer::Nv12(buffer) => buffer,
            FrameBuffer::Native(_) => {
                panic!("VideoFrame does not contain NV12 data (native handle output requested)")
            }
        }
    }
}

fn nv12_uv_rows(height: u32) -> usize {
    (height as usize).div_ceil(2)
}

#[derive(Debug, Error)]
pub enum DecoderError {
    #[error("backend {backend} is not supported in this build")]
    Unsupported { backend: &'static str },

    #[error("{backend} backend failed: {message}")]
    BackendFailure {
        backend: &'static str,
        message: String,
    },

    #[error("configuration error: {message}")]
    Configuration { message: String },

    #[error("invalid frame: {reason}")]
    InvalidFrame { reason: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl DecoderError {
    pub fn unsupported(backend: &'static str) -> Self {
        Self::Unsupported { backend }
    }

    pub fn backend_failure(backend: &'static str, message: impl Into<String>) -> Self {
        Self::BackendFailure {
            backend,
            message: message.into(),
        }
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self::Configuration {
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoiConfig {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectionRegion {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubtitleDetectionResult {
    pub has_subtitle: bool,
    pub max_score: f32,
    pub regions: Vec<DetectionRegion>,
}

impl SubtitleDetectionResult {
    pub fn empty() -> Self {
        Self {
            has_subtitle: false,
            max_score: 0.0,
            regions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OcrRegion {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl OcrRegion {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OcrText {
    pub region: OcrRegion,
    pub text: String,
    pub confidence: Option<f32>,
}

impl OcrText {
    pub fn new(region: OcrRegion, text: String) -> Self {
        Self {
            region,
            text,
            confidence: None,
        }
    }

    pub fn with_confidence(mut self, value: f32) -> Self {
        self.confidence = Some(value);
        self
    }
}

#[derive(Debug, Clone)]
pub struct OcrResponse {
    pub texts: Vec<OcrText>,
}

impl OcrResponse {
    pub fn new(texts: Vec<OcrText>) -> Self {
        Self { texts }
    }

    pub fn empty() -> Self {
        Self { texts: Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sample_nv12_frame() -> VideoFrame {
        VideoFrame::from_nv12_owned(
            4,
            3,
            4,
            4,
            Some(Duration::from_millis(10)),
            Some(Duration::from_millis(8)),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
            vec![10, 11, 12, 13, 14, 15, 16, 17],
        )
        .expect("valid nv12 frame")
    }

    #[test]
    fn from_nv12_owned_truncates_extra_plane_bytes() {
        let frame = VideoFrame::from_nv12_owned(4, 3, 4, 4, None, None, vec![1; 16], vec![2; 12])
            .expect("frame must be created");

        assert_eq!(frame.width(), 4);
        assert_eq!(frame.height(), 3);
        assert_eq!(frame.y_plane().len(), 12);
        assert_eq!(frame.uv_plane().len(), 8);
        assert_eq!(frame.stride(), 4);
        assert_eq!(frame.y_stride(), 4);
        assert_eq!(frame.uv_stride(), 4);
    }

    #[test]
    fn from_nv12_owned_rejects_short_planes() {
        let y_err = VideoFrame::from_nv12_owned(4, 3, 4, 4, None, None, vec![1; 11], vec![2; 8])
            .expect_err("must reject insufficient y plane");
        assert!(matches!(y_err, DecoderError::InvalidFrame { .. }));
        assert!(
            y_err
                .to_string()
                .contains("insufficient NV12 Y plane bytes")
        );

        let uv_err = VideoFrame::from_nv12_owned(4, 3, 4, 4, None, None, vec![1; 12], vec![2; 7])
            .expect_err("must reject insufficient uv plane");
        assert!(matches!(uv_err, DecoderError::InvalidFrame { .. }));
        assert!(
            uv_err
                .to_string()
                .contains("insufficient NV12 UV plane bytes")
        );
    }

    #[test]
    fn metadata_mutators_update_fields() {
        let mut frame = sample_nv12_frame()
            .with_serial(7)
            .with_index(Some(3))
            .with_pts(Some(Duration::from_millis(100)))
            .with_dts(Some(Duration::from_millis(90)));

        assert_eq!(frame.serial(), 7);
        assert_eq!(frame.index(), Some(3));
        assert_eq!(frame.pts(), Some(Duration::from_millis(100)));
        assert_eq!(frame.dts(), Some(Duration::from_millis(90)));

        frame.set_serial(9);
        frame.set_index(Some(5));
        frame.set_pts(None);
        frame.set_dts(None);

        assert_eq!(frame.serial(), 9);
        assert_eq!(frame.index(), Some(5));
        assert_eq!(frame.pts(), None);
        assert_eq!(frame.dts(), None);
    }

    static RELEASE_COUNT: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn release_counting(ptr: *mut c_void) {
        RELEASE_COUNT.fetch_add(1, Ordering::SeqCst);
        if !ptr.is_null() {
            unsafe {
                drop(Box::from_raw(ptr as *mut u8));
            }
        }
    }

    #[test]
    fn native_handle_release_called_once_after_last_drop() {
        RELEASE_COUNT.store(0, Ordering::SeqCst);
        let handle = Box::into_raw(Box::new(123_u8)) as *mut c_void;
        let frame = VideoFrame::from_native_handle(
            16,
            9,
            None,
            None,
            Some(42),
            "mock-native",
            100,
            handle,
            release_counting,
        )
        .expect("native frame should be created");

        let clone = frame.clone();
        assert_eq!(RELEASE_COUNT.load(Ordering::SeqCst), 0);
        assert_eq!(
            frame.native().expect("must be native").backend(),
            "mock-native"
        );
        assert_eq!(frame.native().expect("must be native").pixel_format(), 100);

        drop(frame);
        assert_eq!(RELEASE_COUNT.load(Ordering::SeqCst), 0);
        drop(clone);
        assert_eq!(RELEASE_COUNT.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn from_native_handle_rejects_null_pointer() {
        let err = VideoFrame::from_native_handle(
            1,
            1,
            None,
            None,
            None,
            "noop",
            0,
            std::ptr::null_mut(),
            release_counting,
        )
        .expect_err("null handle should fail");
        assert!(matches!(err, DecoderError::InvalidFrame { .. }));
        assert!(err.to_string().contains("native handle is null"));
    }

    #[test]
    fn decoder_error_helper_constructors_are_stable() {
        let unsupported = DecoderError::unsupported("ffmpeg");
        assert!(unsupported.to_string().contains("not supported"));

        let backend = DecoderError::backend_failure("decoder", "crashed");
        assert!(
            backend
                .to_string()
                .contains("decoder backend failed: crashed")
        );

        let config = DecoderError::configuration("bad config");
        assert!(
            config
                .to_string()
                .contains("configuration error: bad config")
        );
    }

    #[test]
    fn subtitle_detection_and_ocr_defaults_are_empty() {
        let detection = SubtitleDetectionResult::empty();
        assert!(!detection.has_subtitle);
        assert_eq!(detection.max_score, 0.0);
        assert!(detection.regions.is_empty());

        let text = OcrText::new(OcrRegion::new(0.1, 0.2, 0.3, 0.4), "hello".to_string())
            .with_confidence(0.95);
        assert_eq!(text.text, "hello");
        assert_eq!(text.confidence, Some(0.95));

        let response = OcrResponse::new(vec![text]);
        assert_eq!(response.texts.len(), 1);
        assert!(OcrResponse::empty().texts.is_empty());
    }
}
