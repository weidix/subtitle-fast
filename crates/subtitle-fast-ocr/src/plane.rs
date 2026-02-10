use std::fmt;
use std::ops::Deref;

use subtitle_fast_types::VideoFrame;

use crate::error::OcrError;

/// Immutable view over a Y (luminance) plane.
#[derive(Clone)]
pub struct LumaPlane<'a> {
    width: u32,
    height: u32,
    stride: usize,
    data: &'a [u8],
}

impl<'a> LumaPlane<'a> {
    pub fn from_parts(
        width: u32,
        height: u32,
        stride: usize,
        data: &'a [u8],
    ) -> Result<Self, OcrError> {
        let required = stride
            .checked_mul(height as usize)
            .ok_or(OcrError::PlaneOverflow { stride, height })?;
        if data.len() < required {
            return Err(OcrError::InsufficientPlaneData {
                provided: data.len(),
                required,
            });
        }
        Ok(Self {
            width,
            height,
            stride,
            data: &data[..required],
        })
    }

    pub fn from_frame(frame: &'a VideoFrame) -> Self {
        // SAFETY: VideoFrame guarantees the buffer is at least stride * height bytes long.
        Self {
            width: frame.width(),
            height: frame.height(),
            stride: frame.stride(),
            data: frame.data(),
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn stride(&self) -> usize {
        self.stride
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }
}

impl fmt::Debug for LumaPlane<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LumaPlane")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("stride", &self.stride)
            .field("bytes", &self.data.len())
            .finish()
    }
}

impl Deref for LumaPlane<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use subtitle_fast_types::VideoFrame;

    #[test]
    fn from_parts_clips_to_stride_times_height() {
        let data = vec![1u8; 20];
        let plane = LumaPlane::from_parts(4, 3, 5, &data).expect("plane");
        assert_eq!(plane.width(), 4);
        assert_eq!(plane.height(), 3);
        assert_eq!(plane.stride(), 5);
        assert_eq!(plane.data().len(), 15);
    }

    #[test]
    fn from_parts_rejects_insufficient_data() {
        let data = vec![1u8; 14];
        let err = LumaPlane::from_parts(4, 3, 5, &data).expect_err("must fail");
        assert!(matches!(err, OcrError::InsufficientPlaneData { .. }));
    }

    #[test]
    fn from_frame_matches_video_frame_shape() {
        let frame = VideoFrame::from_nv12_owned(4, 3, 4, 4, None, None, vec![0; 12], vec![128; 8])
            .expect("frame");
        let plane = LumaPlane::from_frame(&frame);
        assert_eq!(plane.width(), 4);
        assert_eq!(plane.height(), 3);
        assert_eq!(plane.stride(), 4);
        assert_eq!(plane.deref().len(), 12);
    }
}
