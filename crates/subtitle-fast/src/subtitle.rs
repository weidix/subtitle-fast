use std::fmt::Write as _;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct SubtitleLine {
    pub center: f32,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct MergedSubtitle {
    pub id: u64,
    pub start_time: Duration,
    pub end_time: Duration,
    pub start_frame: u64,
    pub lines: Vec<SubtitleLine>,
}

#[derive(Clone, Debug)]
pub struct TimedSubtitle {
    pub id: u64,
    pub start_ms: f64,
    pub end_ms: f64,
    pub lines: Vec<String>,
}

impl MergedSubtitle {
    pub fn as_timed(&self) -> TimedSubtitle {
        TimedSubtitle {
            id: self.id,
            start_ms: self.start_time.as_secs_f64() * 1000.0,
            end_ms: self.end_time.as_secs_f64() * 1000.0,
            lines: ordered_lines(&self.lines),
        }
    }
}

impl TimedSubtitle {
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }
}

pub fn sort_subtitles(subtitles: &mut [MergedSubtitle]) {
    subtitles.sort_by(|a, b| match a.start_time.cmp(&b.start_time) {
        std::cmp::Ordering::Equal => a.start_frame.cmp(&b.start_frame),
        other => other,
    });
}

pub fn render_srt(subtitles: &[MergedSubtitle]) -> String {
    let mut output = String::new();
    let mut cue_index = 1usize;
    for cue in subtitles {
        let lines = ordered_lines(&cue.lines);
        if lines.is_empty() {
            continue;
        }
        if cue_index > 1 {
            output.push('\n');
        }
        let _ = writeln!(&mut output, "{cue_index}");
        let _ = writeln!(
            &mut output,
            "{} --> {}",
            format_timestamp(cue.start_time),
            format_timestamp(cue.end_time)
        );
        for line in lines {
            let _ = writeln!(&mut output, "{line}");
        }
        cue_index += 1;
    }
    output
}

fn ordered_lines(lines: &[SubtitleLine]) -> Vec<String> {
    let mut refs: Vec<&SubtitleLine> = lines.iter().collect();
    refs.sort_by(|a, b| {
        a.center
            .partial_cmp(&b.center)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut ordered = Vec::new();
    for line in refs {
        let text = line.text.trim();
        if text.is_empty() {
            continue;
        }
        if ordered.last().is_some_and(|last: &String| last == text) {
            continue;
        }
        ordered.push(text.to_string());
    }
    ordered
}

fn format_timestamp(time: Duration) -> String {
    let millis = time
        .as_secs()
        .saturating_mul(1000)
        .saturating_add(u64::from(time.subsec_millis()));
    let hours = millis / 3_600_000;
    let minutes = (millis % 3_600_000) / 60_000;
    let seconds = (millis % 60_000) / 1000;
    let remain_ms = millis % 1000;
    format!("{hours:02}:{minutes:02}:{seconds:02},{remain_ms:03}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subtitle_with_lines(
        id: u64,
        start_ms: u64,
        end_ms: u64,
        start_frame: u64,
        lines: Vec<(f32, &str)>,
    ) -> MergedSubtitle {
        MergedSubtitle {
            id,
            start_time: Duration::from_millis(start_ms),
            end_time: Duration::from_millis(end_ms),
            start_frame,
            lines: lines
                .into_iter()
                .map(|(center, text)| SubtitleLine {
                    center,
                    text: text.to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn as_timed_orders_lines_by_center_and_deduplicates() {
        let subtitle = subtitle_with_lines(
            7,
            100,
            250,
            10,
            vec![(0.8, "  World "), (0.2, " Hello "), (0.6, "Hello")],
        );

        let timed = subtitle.as_timed();
        assert_eq!(timed.id, 7);
        assert!((timed.start_ms - 100.0).abs() < f64::EPSILON);
        assert!((timed.end_ms - 250.0).abs() < f64::EPSILON);
        assert_eq!(timed.lines, vec!["Hello".to_string(), "World".to_string()]);
        assert_eq!(timed.text(), "Hello\nWorld");
    }

    #[test]
    fn sort_subtitles_orders_by_start_then_frame() {
        let mut subtitles = vec![
            subtitle_with_lines(1, 300, 400, 30, vec![(0.1, "a")]),
            subtitle_with_lines(2, 100, 200, 20, vec![(0.1, "b")]),
            subtitle_with_lines(3, 100, 250, 10, vec![(0.1, "c")]),
        ];

        sort_subtitles(&mut subtitles);
        assert_eq!(subtitles[0].id, 3);
        assert_eq!(subtitles[1].id, 2);
        assert_eq!(subtitles[2].id, 1);
    }

    #[test]
    fn render_srt_skips_empty_cues_and_keeps_contiguous_indices() {
        let subtitles = vec![
            subtitle_with_lines(1, 0, 1000, 0, vec![(0.3, "   ")]),
            subtitle_with_lines(2, 1000, 2000, 1, vec![(0.3, "Line A")]),
            subtitle_with_lines(3, 2000, 3000, 2, vec![(0.7, "Line B")]),
        ];

        let srt = render_srt(&subtitles);
        assert!(srt.contains("1\n00:00:01,000 --> 00:00:02,000\nLine A"));
        assert!(srt.contains("2\n00:00:02,000 --> 00:00:03,000\nLine B"));
        assert!(!srt.contains("3\n"));
    }
}
