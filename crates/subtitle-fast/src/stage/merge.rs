use std::time::Duration;

use futures_util::{StreamExt, stream::unfold};
use tokio::sync::mpsc;

use super::StreamBundle;
use super::detector::DetectionSample;
use super::lifecycle::RegionTimings;
use super::ocr::{OcrEvent, OcrStageError, OcrStageResult, OcrTimings};
use crate::subtitle::{MergedSubtitle, SubtitleLine};
use subtitle_fast_ocr::OcrResponse;

const MERGE_CHANNEL_CAPACITY: usize = 4;
const MERGE_GAP: Duration = Duration::from_millis(120);
const SUBTITLE_CACHE_WINDOW: Duration = Duration::from_secs(2);

pub type MergeResult = Result<MergeOutput, OcrStageError>;

pub struct Merge {
    cache_window: Duration,
}

impl Merge {
    pub fn new(cache_window: Duration) -> Self {
        Self { cache_window }
    }

    pub fn with_default_window() -> Self {
        Self::new(SUBTITLE_CACHE_WINDOW)
    }

    pub fn attach(self, input: StreamBundle<OcrStageResult>) -> StreamBundle<MergeResult> {
        let StreamBundle {
            stream,
            total_frames,
        } = input;

        let (tx, rx) = mpsc::channel::<MergeResult>(MERGE_CHANNEL_CAPACITY);
        let cache_window = self.cache_window;

        tokio::spawn(async move {
            let mut upstream = stream;
            let mut worker = MergeWorker::new(cache_window);

            while let Some(event) = upstream.next().await {
                match event {
                    Ok(ocr_event) => {
                        let result = worker.handle_event(ocr_event);
                        if tx.send(Ok(result)).await.is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err)).await;
                        return;
                    }
                }
            }
        });

        let stream = Box::pin(unfold(rx, |mut receiver| async {
            receiver.recv().await.map(|item| (item, receiver))
        }));

        StreamBundle::new(stream, total_frames)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SubtitleStats {
    pub cues: u64,
    pub merged: u64,
    pub ocr_empty: u64,
}

pub struct MergeOutput {
    pub sample: Option<DetectionSample>,
    pub region_timings: Option<RegionTimings>,
    pub ocr_timings: Option<OcrTimings>,
    pub updates: Vec<SubtitleUpdate>,
    pub stats: SubtitleStats,
}

#[derive(Clone, Debug)]
pub struct SubtitleUpdate {
    pub kind: SubtitleUpdateKind,
    pub subtitle: MergedSubtitle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubtitleUpdateKind {
    New,
    Updated,
}

struct SubtitleCue {
    start_time: Duration,
    end_time: Duration,
    start_frame: u64,
    text: String,
    center: f32,
}

struct MergeWorker {
    cache_window: Duration,
    subtitles: Vec<MergedSubtitle>,
    next_id: u64,
    stats: SubtitleStats,
}

impl MergeWorker {
    fn new(cache_window: Duration) -> Self {
        Self {
            cache_window,
            subtitles: Vec::new(),
            next_id: 0,
            stats: SubtitleStats::default(),
        }
    }

    fn handle_event(&mut self, event: OcrEvent) -> MergeOutput {
        let mut updates = Vec::new();

        for subtitle in event.regions {
            let text = normalize_text(&response_to_text(&subtitle.response));
            if text.is_empty() {
                self.stats.ocr_empty = self.stats.ocr_empty.saturating_add(1);
                continue;
            }
            let center = subtitle.region.y + subtitle.region.height * 0.5;
            let cue = SubtitleCue {
                start_time: subtitle.lifecycle.start_time,
                end_time: subtitle.lifecycle.end_time,
                start_frame: subtitle.lifecycle.start_frame,
                text,
                center,
            };
            if let Some(update) = self.apply_cue(cue) {
                updates.push(update);
            }
        }

        MergeOutput {
            sample: event.sample,
            region_timings: event.region_timings,
            ocr_timings: event.timings,
            updates,
            stats: self.stats.clone(),
        }
    }

    fn apply_cue(&mut self, cue: SubtitleCue) -> Option<SubtitleUpdate> {
        self.prune(cue.start_time);

        if let Some(last) = self.subtitles.last_mut()
            && should_merge(last, &cue)
        {
            last.start_time = last.start_time.min(cue.start_time);
            last.end_time = last.end_time.max(cue.end_time);
            last.start_frame = last.start_frame.min(cue.start_frame);
            if !last.lines.iter().any(|line| line.text == cue.text) {
                last.lines.push(SubtitleLine {
                    center: cue.center,
                    text: cue.text.clone(),
                });
            }
            self.stats.merged = self.stats.merged.saturating_add(1);
            return Some(SubtitleUpdate {
                kind: SubtitleUpdateKind::Updated,
                subtitle: last.clone(),
            });
        }

        let subtitle = MergedSubtitle {
            id: self.next_id,
            start_time: cue.start_time,
            end_time: cue.end_time,
            start_frame: cue.start_frame,
            lines: vec![SubtitleLine {
                center: cue.center,
                text: cue.text,
            }],
        };
        self.next_id = self.next_id.saturating_add(1);
        self.stats.cues = self.stats.cues.saturating_add(1);
        self.subtitles.push(subtitle.clone());
        Some(SubtitleUpdate {
            kind: SubtitleUpdateKind::New,
            subtitle,
        })
    }

    fn prune(&mut self, now: Duration) {
        let Some(cutoff) = now.checked_sub(self.cache_window) else {
            return;
        };
        while let Some(first) = self.subtitles.first() {
            if first.end_time < cutoff {
                self.subtitles.remove(0);
            } else {
                break;
            }
        }
    }
}

fn should_merge(current: &MergedSubtitle, incoming: &SubtitleCue) -> bool {
    if incoming.start_time <= current.end_time {
        return true;
    }
    let gap = incoming
        .start_time
        .checked_sub(current.end_time)
        .unwrap_or(Duration::ZERO);
    if gap <= MERGE_GAP {
        current.lines.iter().any(|line| line.text == incoming.text)
    } else {
        false
    }
}

fn response_to_text(response: &OcrResponse) -> String {
    if response.texts.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    for entry in &response.texts {
        let trimmed = entry.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        parts.push(trimmed.to_string());
    }
    parts.join("\n")
}

fn normalize_text(text: &str) -> String {
    text.lines()
        .map(|line| {
            line.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use subtitle_fast_ocr::{OcrRegion, OcrText};

    fn cue(text: &str, start_ms: u64, end_ms: u64, start_frame: u64) -> SubtitleCue {
        SubtitleCue {
            start_time: Duration::from_millis(start_ms),
            end_time: Duration::from_millis(end_ms),
            start_frame,
            text: text.to_string(),
            center: 0.5,
        }
    }

    fn sample_response(lines: &[&str]) -> OcrResponse {
        OcrResponse::new(
            lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    OcrText::new(
                        OcrRegion::new(0.0, i as f32, 10.0, 1.0),
                        (*line).to_string(),
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn normalize_text_collapses_spaces_and_empty_lines() {
        let text = "  hello   world  \n\n foo\tbar ";
        assert_eq!(normalize_text(text), "hello world\nfoo bar");
    }

    #[test]
    fn response_to_text_ignores_blank_entries() {
        let response = sample_response(&["line1", "   ", "line2"]);
        assert_eq!(response_to_text(&response), "line1\nline2");
    }

    #[test]
    fn should_merge_on_overlap_and_small_gap_with_same_text() {
        let current = MergedSubtitle {
            id: 0,
            start_time: Duration::from_millis(0),
            end_time: Duration::from_millis(200),
            start_frame: 0,
            lines: vec![SubtitleLine {
                center: 0.5,
                text: "Hello".to_string(),
            }],
        };

        assert!(should_merge(&current, &cue("Other", 150, 260, 10)));
        assert!(should_merge(&current, &cue("Hello", 260, 330, 15)));
        assert!(!should_merge(&current, &cue("Different", 260, 330, 15)));
        assert!(!should_merge(&current, &cue("Hello", 500, 650, 20)));
    }

    #[test]
    fn apply_cue_creates_and_updates_subtitle_stats() {
        let mut worker = MergeWorker::new(Duration::from_secs(2));

        let first = worker
            .apply_cue(cue("same", 100, 200, 3))
            .expect("first cue");
        assert_eq!(first.kind, SubtitleUpdateKind::New);
        assert_eq!(first.subtitle.id, 0);
        assert_eq!(worker.stats.cues, 1);
        assert_eq!(worker.stats.merged, 0);

        let second = worker
            .apply_cue(cue("same", 250, 320, 2))
            .expect("second cue");
        assert_eq!(second.kind, SubtitleUpdateKind::Updated);
        assert_eq!(second.subtitle.id, 0);
        assert_eq!(second.subtitle.start_frame, 2);
        assert_eq!(worker.stats.cues, 1);
        assert_eq!(worker.stats.merged, 1);
    }

    #[test]
    fn prune_discards_expired_cached_subtitles() {
        let mut worker = MergeWorker::new(Duration::from_millis(200));
        worker.apply_cue(cue("a", 0, 50, 0)).expect("cue1");
        worker.apply_cue(cue("b", 80, 120, 1)).expect("cue2");

        worker.prune(Duration::from_millis(500));
        assert!(worker.subtitles.is_empty());
    }
}
