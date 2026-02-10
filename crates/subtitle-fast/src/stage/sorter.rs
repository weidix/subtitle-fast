use std::collections::BTreeMap;
use std::pin::Pin;

use futures_util::{Stream, StreamExt};

use super::StreamBundle;
use subtitle_fast_types::{DecoderResult, VideoFrame};

pub struct FrameSorter;

impl FrameSorter {
    pub fn new() -> Self {
        Self
    }

    pub fn attach(
        self,
        input: StreamBundle<DecoderResult<VideoFrame>>,
    ) -> StreamBundle<DecoderResult<VideoFrame>> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let state = SorterState {
            upstream: stream,
            pool: FramePool::default(),
            finished: false,
        };

        let stream = Box::pin(futures_util::stream::unfold(state, SorterState::next));
        StreamBundle::new(stream, total_frames)
    }
}

impl Default for FrameSorter {
    fn default() -> Self {
        Self::new()
    }
}

struct SorterState {
    upstream: Pin<Box<dyn Stream<Item = DecoderResult<VideoFrame>> + Send>>,
    pool: FramePool,
    finished: bool,
}

impl SorterState {
    async fn next(mut state: SorterState) -> Option<(DecoderResult<VideoFrame>, SorterState)> {
        loop {
            if let Some(frame) = state.pool.pop_next() {
                return Some((Ok(frame), state));
            }

            if state.finished {
                return None;
            }

            match state.upstream.as_mut().next().await {
                Some(Ok(frame)) => {
                    state.pool.insert(frame);
                }
                Some(Err(err)) => {
                    state.finished = true;
                    return Some((Err(err), state));
                }
                None => {
                    state.finished = true;
                    if let Some(frame) = state.pool.pop_next() {
                        return Some((Ok(frame), state));
                    }
                    return None;
                }
            }
        }
    }
}

#[derive(Default)]
struct FramePool {
    pending: BTreeMap<u64, VideoFrame>,
    fallback_index: u64,
}

impl FramePool {
    fn insert(&mut self, frame: VideoFrame) {
        let key = frame.index().unwrap_or_else(|| {
            let key = self.fallback_index;
            self.fallback_index = self.fallback_index.saturating_add(1);
            key
        });

        self.pending.entry(key).or_insert(frame);
    }

    fn pop_next(&mut self) -> Option<VideoFrame> {
        let key = self.pending.keys().next().copied()?;
        self.pending.remove(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn frame(index: Option<u64>, seed: u8) -> VideoFrame {
        let mut frame = VideoFrame::from_nv12_owned(
            2,
            2,
            2,
            2,
            Some(Duration::from_millis(seed as u64)),
            None,
            vec![seed; 4],
            vec![128; 2],
        )
        .expect("frame");
        frame.set_index(index);
        frame
    }

    #[test]
    fn frame_pool_returns_lowest_index_first() {
        let mut pool = FramePool::default();
        pool.insert(frame(Some(5), 5));
        pool.insert(frame(Some(2), 2));
        pool.insert(frame(Some(4), 4));

        assert_eq!(pool.pop_next().expect("first").index(), Some(2));
        assert_eq!(pool.pop_next().expect("second").index(), Some(4));
        assert_eq!(pool.pop_next().expect("third").index(), Some(5));
        assert!(pool.pop_next().is_none());
    }

    #[test]
    fn frame_pool_uses_fallback_index_for_missing_indices() {
        let mut pool = FramePool::default();
        pool.insert(frame(None, 1));
        pool.insert(frame(None, 2));

        let first = pool.pop_next().expect("first");
        let second = pool.pop_next().expect("second");
        assert_eq!(first.index(), None);
        assert_eq!(second.index(), None);
        assert_eq!(first.data(), &[1, 1, 1, 1]);
        assert_eq!(second.data(), &[2, 2, 2, 2]);
    }

    #[test]
    fn frame_pool_keeps_first_frame_for_duplicate_index() {
        let mut pool = FramePool::default();
        pool.insert(frame(Some(3), 7));
        pool.insert(frame(Some(3), 9));

        let item = pool.pop_next().expect("entry");
        assert_eq!(item.index(), Some(3));
        assert_eq!(item.data(), &[7, 7, 7, 7]);
        assert!(pool.pop_next().is_none());
    }
}
