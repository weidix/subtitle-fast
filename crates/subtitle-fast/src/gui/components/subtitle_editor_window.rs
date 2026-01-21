use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, DispatchPhase, Div, Entity, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, ScrollHandle, SharedString, Subscription, Task, Window,
    WindowBounds, WindowDecorations, WindowOptions, div, hsla, point, px, rgb, size,
};

use crate::gui::components::detection_sidebar::{SubtitleEdit, SubtitleMessage};
use crate::gui::components::inputs::{InputKind, TextInput};
use crate::gui::components::video_player::VideoOpenOptions;
use crate::gui::components::video_player::VideoPlayerInfoSnapshot;
use crate::gui::components::{
    DetectionHandle, Titlebar, VideoPlayer, VideoPlayerControlHandle, VideoPlayerInfoHandle,
};
use crate::gui::icons::{Icon, icon_sm};
use crate::gui::session::VideoSession;
use crate::subtitle::TimedSubtitle;

const PREVIEW_SEEK_OFFSET_MS: f64 = 100.0;
const LIST_MIN_WIDTH: f32 = 320.0;
const DEFAULT_PREVIEW_ASPECT: f32 = 16.0 / 9.0;
const LIST_ROW_PADDING_Y: f32 = 8.0;
const LIST_ROW_GAP: f32 = 4.0;
const LIST_ROW_LINE_GAP: f32 = 2.0;
const LIST_ROW_TIME_TEXT_SIZE: f32 = 10.0;
const LIST_ROW_TEXT_SIZE: f32 = 12.0;
const LIST_ROW_TIME_HEIGHT: f32 = 16.0;
const LIST_ROW_BADGE_HEIGHT: f32 = 16.0;
const LIST_ESTIMATED_BODY_LINES: f32 = 2.0;
const LIST_PADDING_TOP: f32 = 4.0;
const LIST_PADDING_BOTTOM: f32 = 4.0;
const LIST_HEIGHT_EPS: f32 = 0.5;
const LIST_OVERSCAN_PX: f32 = 160.0;
const LIST_INITIAL_RENDER_COUNT: usize = 50;
const DEFAULT_ESTIMATED_ROW_HEIGHT: f32 = 60.0;
const SCROLLBAR_ANIMATION_MS: u64 = 180;
const SCROLLBAR_SETTLE_EPS: f32 = 1.0;
const SCROLLBAR_CANCEL_EPS: f32 = 1.0;
const SCROLLBAR_FADE_DELAY_MS: u64 = 1000;
const SCROLLBAR_FADE_MS: u64 = 200;
const LINE_PLACEHOLDER: &str = "Subtitle line";
const TRAILING_ICON_BUTTON_WIDTH: f32 = 34.0;
const TRAILING_ICON_BUTTON_HEIGHT: f32 = 28.0;
const TIME_INPUT_TRAILING_WIDTH: f32 = TRAILING_ICON_BUTTON_WIDTH;
const TIME_INPUT_TRAILING_GAP: f32 = 3.0;
const TRAILING_ICON_BUTTON_GAP: f32 = 4.0;
const LINE_INPUT_TRAILING_WIDTH: f32 = TRAILING_ICON_BUTTON_WIDTH * 2.0 + TRAILING_ICON_BUTTON_GAP;
const TIME_COMPARE_EPS: f64 = 1e-6;

#[derive(Clone, Debug)]
struct EditableSubtitle {
    id: u64,
    start_ms: f64,
    end_ms: f64,
    lines: Vec<String>,
}

#[derive(Clone, Debug)]
struct SelectedSnapshot {
    id: u64,
    start_seconds: f64,
    end_seconds: f64,
    lines: Vec<String>,
}

#[derive(Clone, Debug)]
struct DraftLine {
    text: String,
    deleted: bool,
}

#[derive(Clone, Debug)]
struct SubtitleDraft {
    start_text: String,
    end_text: String,
    lines: Vec<DraftLine>,
}

#[derive(Clone, Copy, Debug)]
enum TimeField {
    Start,
    End,
}

#[derive(Clone)]
struct LineInputState {
    input: Entity<TextInput>,
    deleted: bool,
}

impl EditableSubtitle {
    fn from_timed(subtitle: TimedSubtitle) -> Self {
        Self {
            id: subtitle.id,
            start_ms: subtitle.start_ms,
            end_ms: subtitle.end_ms,
            lines: normalize_lines(&subtitle.lines),
        }
    }

    fn search_text(&self) -> String {
        if self.lines.is_empty() {
            return String::new();
        }
        self.lines.join(" ")
    }
}

#[derive(Clone)]
struct StatusMessage {
    text: SharedString,
    is_error: bool,
}

#[derive(Clone, Copy, Debug)]
struct ScrollbarMetrics {
    viewport_height: f32,
    max_offset: f32,
    scroll_top: f32,
    thumb_height: f32,
    thumb_top: f32,
}

#[derive(Clone, Copy, Debug)]
struct ScrollbarDragState {
    start_pointer_y: f32,
    start_scroll_top: f32,
    viewport_height: f32,
    max_offset: f32,
    thumb_height: f32,
}

#[derive(Clone, Copy, Debug)]
struct ScrollbarAnimation {
    start: ScrollbarMetrics,
    target: ScrollbarMetrics,
    started_at: Instant,
}

pub struct SubtitleEditorWindow {
    detection: DetectionHandle,
    video_path: PathBuf,
    subtitles: Vec<EditableSubtitle>,
    subtitle_task: Option<Task<()>>,
    search_input: Entity<TextInput>,
    start_input: Entity<TextInput>,
    end_input: Entity<TextInput>,
    line_inputs: Vec<LineInputState>,
    line_input_subscriptions: Vec<Subscription>,
    search_query: SharedString,
    selected_id: Option<u64>,
    selected_snapshot: Option<SelectedSnapshot>,
    drafts: HashMap<u64, SubtitleDraft>,
    dirty: bool,
    suppress_input_observers: bool,
    status: Option<StatusMessage>,
    subscriptions: Vec<Subscription>,
    titlebar: Entity<Titlebar>,
    player: Entity<VideoPlayer>,
    player_control: VideoPlayerControlHandle,
    player_info: VideoPlayerInfoHandle,
    list_scroll_handle: ScrollHandle,
    list_row_heights: Vec<Pixels>,
    list_row_offsets: Vec<Pixels>,
    list_row_measured: Vec<bool>,
    list_estimated_row_height: Pixels,
    list_scroll_refresh_pending: bool,
    list_scroll_drag: Option<ScrollbarDragState>,
    list_scroll_settle_pending: bool,
    list_scrollbar_animation: Option<ScrollbarAnimation>,
    list_last_scrollbar_metrics: Option<ScrollbarMetrics>,
    list_scrollbar_hovered: bool,
    list_scrollbar_last_interaction: Option<Instant>,
}

impl SubtitleEditorWindow {
    pub fn open(session: VideoSession, cx: &mut App) -> Option<gpui::WindowHandle<Self>> {
        let title = subtitle_editor_title(&session.label);
        let title_for_bar = title.clone();
        let title_for_window = title.clone();
        let bounds = Bounds::centered(None, size(px(980.0), px(720.0)), cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(820.0), px(600.0))),
                    is_resizable: true,
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some(title_for_window),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    window_decorations: Some(WindowDecorations::Client),
                    ..Default::default()
                },
                move |_, cx| {
                    cx.new(|cx| {
                        SubtitleEditorWindow::new(session.clone(), title_for_bar.clone(), cx)
                    })
                },
            )
            .ok()?;

        let _ = handle.update(cx, |_, window, _| {
            window.activate_window();
        });

        Some(handle)
    }

    fn new(session: VideoSession, title: SharedString, cx: &mut Context<Self>) -> Self {
        let titlebar = cx.new(|_| Titlebar::new("subtitle-editor-titlebar", title));
        let search_input = cx.new(|cx| {
            TextInput::new(cx, "Search subtitles or time...", InputKind::Text)
                .with_leading_icon(Icon::Search)
        });
        let start_input = cx.new(|cx| TextInput::new(cx, "Start s", InputKind::Float));
        let end_input = cx.new(|cx| TextInput::new(cx, "End s", InputKind::Float));
        let line_inputs = vec![Self::build_line_input(cx)];
        let (player, control, info) = VideoPlayer::new();
        let player = cx.new(|_| player);
        let subtitles = session
            .detection
            .subtitles_snapshot()
            .into_iter()
            .map(EditableSubtitle::from_timed)
            .collect::<Vec<_>>();

        let estimated_row_height = px(DEFAULT_ESTIMATED_ROW_HEIGHT);
        let mut list_row_heights = Vec::with_capacity(subtitles.len());
        let mut list_row_offsets = Vec::with_capacity(subtitles.len() + 1);
        let mut list_row_measured = Vec::with_capacity(subtitles.len());
        list_row_offsets.push(Pixels::ZERO);
        for _ in 0..subtitles.len() {
            list_row_heights.push(estimated_row_height);
            list_row_measured.push(false);
            if let Some(last) = list_row_offsets.last().copied() {
                list_row_offsets.push(last + estimated_row_height);
            }
        }

        let mut window = Self {
            detection: session.detection,
            video_path: session.path,
            subtitles,
            subtitle_task: None,
            search_input,
            start_input,
            end_input,
            line_inputs,
            line_input_subscriptions: Vec::new(),
            search_query: SharedString::from(""),
            selected_id: None,
            selected_snapshot: None,
            drafts: HashMap::new(),
            dirty: false,
            suppress_input_observers: false,
            status: None,
            subscriptions: Vec::new(),
            titlebar,
            player,
            player_control: control,
            player_info: info,
            list_scroll_handle: ScrollHandle::new(),
            list_row_heights,
            list_row_offsets,
            list_row_measured,
            list_estimated_row_height: estimated_row_height,
            list_scroll_refresh_pending: true,
            list_scroll_drag: None,
            list_scroll_settle_pending: false,
            list_scrollbar_animation: None,
            list_last_scrollbar_metrics: None,
            list_scrollbar_hovered: false,
            list_scrollbar_last_interaction: None,
        };

        window.register_input_observers(cx);
        window.select_initial_subtitle(cx);
        if window.video_path.exists() {
            window.open_video();
        } else {
            window.status = Some(StatusMessage {
                text: "Video file not found for this task.".into(),
                is_error: true,
            });
        }
        window
    }

    fn register_input_observers(&mut self, cx: &mut Context<Self>) {
        let search = self.search_input.clone();
        self.subscriptions
            .push(cx.observe(&search, |this, input, cx| {
                let next = input.read(cx).text();
                if this.search_query != next {
                    this.search_query = next;
                    cx.notify();
                }
            }));

        let start = self.start_input.clone();
        self.subscriptions
            .push(cx.observe(&start, |this, _input, cx| {
                if this.suppress_input_observers {
                    return;
                }
                this.refresh_dirty_state(cx);
                cx.notify();
            }));

        let end = self.end_input.clone();
        self.subscriptions
            .push(cx.observe(&end, |this, _input, cx| {
                if this.suppress_input_observers {
                    return;
                }
                this.refresh_dirty_state(cx);
                cx.notify();
            }));

        self.register_line_observers(cx);
    }

    fn register_line_observers(&mut self, cx: &mut Context<Self>) {
        self.line_input_subscriptions.clear();
        for line in &self.line_inputs {
            let input = line.input.clone();
            self.line_input_subscriptions
                .push(cx.observe(&input, |this, _input, cx| {
                    if this.suppress_input_observers {
                        return;
                    }
                    this.refresh_dirty_state(cx);
                    cx.notify();
                }));
        }
    }

    fn select_initial_subtitle(&mut self, cx: &mut Context<Self>) {
        if self.selected_id.is_some() {
            return;
        }

        let Some(entry) = self.subtitles.iter().min_by(|left, right| {
            left.start_ms
                .partial_cmp(&right.start_ms)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        }) else {
            return;
        };

        self.load_selected(entry.id, cx);
    }

    fn set_selected_snapshot(&mut self, id: u64, start_ms: f64, end_ms: f64, lines: Vec<String>) {
        self.selected_snapshot = Some(SelectedSnapshot {
            id,
            start_seconds: normalize_seconds(start_ms / 1000.0),
            end_seconds: normalize_seconds(end_ms / 1000.0),
            lines: normalize_lines(&lines),
        });
    }

    fn select_subtitle(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.selected_id == Some(id) {
            return;
        }
        self.sync_current_draft_state(self.calculate_dirty(cx), cx);
        self.load_selected(id, cx);
    }

    fn sync_current_draft_state(&mut self, is_dirty: bool, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id else {
            return;
        };
        if is_dirty {
            let draft = self.collect_draft(cx);
            self.drafts.insert(id, draft);
        } else {
            self.drafts.remove(&id);
        }
    }

    fn collect_draft(&self, cx: &mut Context<Self>) -> SubtitleDraft {
        let start_text = self.start_input.read(cx).text().to_string();
        let end_text = self.end_input.read(cx).text().to_string();
        let mut lines = Vec::new();
        for line in &self.line_inputs {
            let text = line.input.read(cx).text().to_string();
            lines.push(DraftLine {
                text,
                deleted: line.deleted,
            });
        }
        if lines.is_empty() {
            lines.push(DraftLine {
                text: String::new(),
                deleted: false,
            });
        }
        SubtitleDraft {
            start_text,
            end_text,
            lines,
        }
    }

    fn refresh_dirty_state(&mut self, cx: &mut Context<Self>) {
        let next = self.calculate_dirty(cx);
        if self.dirty != next {
            self.dirty = next;
        }
        self.sync_current_draft_state(next, cx);
    }

    fn calculate_dirty(&self, cx: &mut Context<Self>) -> bool {
        let Some(snapshot) = self.selected_snapshot.as_ref() else {
            return false;
        };
        if self.selected_id != Some(snapshot.id) {
            return false;
        }

        if self.time_field_modified_for_snapshot(TimeField::Start, snapshot, cx) {
            return true;
        }
        if self.time_field_modified_for_snapshot(TimeField::End, snapshot, cx) {
            return true;
        }

        let lines = self.collect_lines(cx);
        lines != snapshot.lines
    }

    fn time_field_modified_for_snapshot(
        &self,
        field: TimeField,
        snapshot: &SelectedSnapshot,
        cx: &mut Context<Self>,
    ) -> bool {
        let original = match field {
            TimeField::Start => snapshot.start_seconds,
            TimeField::End => snapshot.end_seconds,
        };
        let value = match field {
            TimeField::Start => self.start_input.read(cx).text(),
            TimeField::End => self.end_input.read(cx).text(),
        };
        let Some(parsed) = parse_seconds_input(value) else {
            return true;
        };
        (parsed - original).abs() > TIME_COMPARE_EPS
    }

    fn line_is_modified(
        &self,
        index: usize,
        input_state: &LineInputState,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.line_is_new(index) {
            return false;
        }
        let Some(snapshot) = self.selected_snapshot.as_ref() else {
            return false;
        };
        if self.selected_id != Some(snapshot.id) {
            return false;
        }

        let original = snapshot.lines.get(index);
        if input_state.deleted {
            return original.is_some();
        }

        let current = input_state.input.read(cx).text();
        let trimmed = current.as_ref().trim();
        if trimmed.is_empty() {
            return original.is_some();
        }

        match original {
            Some(value) => value != trimmed,
            None => true,
        }
    }

    fn line_is_new(&self, index: usize) -> bool {
        let Some(snapshot) = self.selected_snapshot.as_ref() else {
            return false;
        };
        if self.selected_id != Some(snapshot.id) {
            return false;
        }
        snapshot.lines.get(index).is_none()
    }

    fn time_field_modified(&self, field: TimeField, cx: &mut Context<Self>) -> bool {
        let Some(snapshot) = self.selected_snapshot.as_ref() else {
            return false;
        };
        if self.selected_id != Some(snapshot.id) {
            return false;
        }
        self.time_field_modified_for_snapshot(field, snapshot, cx)
    }

    fn rollback_time_field(&mut self, field: TimeField, cx: &mut Context<Self>) {
        let Some(snapshot) = self.selected_snapshot.as_ref() else {
            return;
        };
        if self.selected_id != Some(snapshot.id) {
            return;
        }
        let target = match field {
            TimeField::Start => snapshot.start_seconds,
            TimeField::End => snapshot.end_seconds,
        };
        let text = format_seconds_value(target);
        self.suppress_input_observers = true;
        match field {
            TimeField::Start => {
                self.start_input.update(cx, |input: &mut TextInput, cx| {
                    input.set_text(text.clone(), cx);
                });
            }
            TimeField::End => {
                self.end_input.update(cx, |input: &mut TextInput, cx| {
                    input.set_text(text.clone(), cx);
                });
            }
        }
        self.suppress_input_observers = false;
        self.refresh_dirty_state(cx);
        cx.notify();
    }

    fn rollback_line(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.line_inputs.len() {
            return;
        }
        let Some(snapshot) = self.selected_snapshot.as_ref() else {
            return;
        };

        self.suppress_input_observers = true;
        if let Some(original) = snapshot.lines.get(index).cloned() {
            if let Some(line) = self.line_inputs.get_mut(index) {
                line.deleted = false;
                line.input.update(cx, |input: &mut TextInput, cx| {
                    input.set_text(original, cx);
                });
            }
        } else if self.line_inputs.len() > 1 {
            self.line_inputs.remove(index);
        } else if let Some(line) = self.line_inputs.get_mut(index) {
            line.deleted = false;
            line.input.update(cx, |input: &mut TextInput, cx| {
                input.set_text("", cx);
            });
        }
        self.suppress_input_observers = false;

        self.register_line_observers(cx);
        self.refresh_dirty_state(cx);
        cx.notify();
    }

    fn build_line_input(cx: &mut Context<Self>) -> LineInputState {
        LineInputState {
            input: cx.new(|cx| TextInput::new(cx, LINE_PLACEHOLDER, InputKind::Text)),
            deleted: false,
        }
    }

    fn replace_line_inputs(&mut self, lines: Vec<String>, cx: &mut Context<Self>) {
        let mut normalized = normalize_lines(&lines);
        if normalized.is_empty() {
            normalized.push(String::new());
        }

        self.suppress_input_observers = true;
        self.line_inputs.clear();
        self.line_input_subscriptions.clear();

        for line in normalized {
            let input_state = Self::build_line_input(cx);
            input_state.input.update(cx, |input: &mut TextInput, cx| {
                input.set_text(line.clone(), cx);
            });
            self.line_inputs.push(input_state);
        }

        self.register_line_observers(cx);
        self.suppress_input_observers = false;
    }

    fn replace_line_inputs_with_draft(&mut self, lines: Vec<DraftLine>, cx: &mut Context<Self>) {
        let mut draft_lines = lines;
        if draft_lines.is_empty() {
            draft_lines.push(DraftLine {
                text: String::new(),
                deleted: false,
            });
        }

        self.suppress_input_observers = true;
        self.line_inputs.clear();
        self.line_input_subscriptions.clear();

        for line in draft_lines {
            let mut input_state = Self::build_line_input(cx);
            input_state.deleted = line.deleted;
            input_state.input.update(cx, |input: &mut TextInput, cx| {
                input.set_text(line.text.clone(), cx);
            });
            self.line_inputs.push(input_state);
        }

        self.register_line_observers(cx);
        self.suppress_input_observers = false;
    }

    fn add_line_input(&mut self, cx: &mut Context<Self>) {
        let input_state = Self::build_line_input(cx);
        self.line_inputs.push(input_state);
        self.register_line_observers(cx);
        self.refresh_dirty_state(cx);
        cx.notify();
    }

    fn toggle_line_deleted(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.should_remove_new_line(index) {
            if index < self.line_inputs.len() {
                self.line_inputs.remove(index);
            }
            self.register_line_observers(cx);
            self.refresh_dirty_state(cx);
            cx.notify();
            return;
        }

        let Some(line) = self.line_inputs.get_mut(index) else {
            return;
        };
        line.deleted = !line.deleted;
        self.refresh_dirty_state(cx);
        cx.notify();
    }

    fn should_remove_new_line(&self, index: usize) -> bool {
        self.line_is_new(index)
    }

    fn open_video(&mut self) {
        if !self.video_path.exists() {
            return;
        }
        self.player_control
            .open_with(self.video_path.clone(), VideoOpenOptions::paused());
    }

    fn ensure_subtitle_listener(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.subtitle_task.is_some() {
            return;
        }

        let handle = cx.entity();
        let mut subtitle_rx = self.detection.subscribe_subtitles();
        let task = window.spawn(cx, async move |cx| {
            while let Some(message) = subtitle_rx.next().await {
                if cx
                    .update(|_window, cx| {
                        handle.update(cx, |this, cx| {
                            this.apply_message(message, cx);
                            cx.notify();
                        });
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.subtitle_task = Some(task);
    }

    fn apply_message(&mut self, message: SubtitleMessage, cx: &mut Context<Self>) {
        match message {
            SubtitleMessage::Reset => {
                self.subtitles.clear();
                self.selected_id = None;
                self.dirty = false;
                self.drafts.clear();
                self.status = None;
                self.clear_inputs(cx);
            }
            SubtitleMessage::New(subtitle) => {
                self.subtitles.push(EditableSubtitle::from_timed(subtitle));
            }
            SubtitleMessage::Updated(subtitle) => {
                let updated = EditableSubtitle::from_timed(subtitle);
                let updated_id = updated.id;
                if let Some(entry) = self
                    .subtitles
                    .iter_mut()
                    .find(|entry| entry.id == updated_id)
                {
                    *entry = updated;
                } else {
                    self.subtitles.push(updated);
                }
                if self.selected_id == Some(updated_id) && !self.dirty {
                    self.load_selected(updated_id, cx);
                }
            }
        }
    }

    fn clear_inputs(&mut self, cx: &mut Context<Self>) {
        self.suppress_input_observers = true;
        self.selected_snapshot = None;
        self.start_input.update(cx, |input: &mut TextInput, cx| {
            input.set_text("", cx);
        });
        self.end_input.update(cx, |input: &mut TextInput, cx| {
            input.set_text("", cx);
        });
        self.replace_line_inputs(Vec::new(), cx);
        self.suppress_input_observers = false;
    }

    fn load_selected(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(entry_index) = self.subtitles.iter().position(|entry| entry.id == id) else {
            self.selected_id = None;
            self.selected_snapshot = None;
            self.drafts.remove(&id);
            self.clear_inputs(cx);
            return;
        };

        let (start_ms, end_ms, lines) = {
            let entry = &self.subtitles[entry_index];
            (entry.start_ms, entry.end_ms, entry.lines.clone())
        };
        let draft = self.drafts.get(&id).cloned();

        self.selected_id = Some(id);
        self.dirty = false;
        self.set_selected_snapshot(id, start_ms, end_ms, lines.clone());
        self.suppress_input_observers = true;
        if let Some(draft) = draft.clone() {
            self.start_input.update(cx, |input: &mut TextInput, cx| {
                input.set_text(draft.start_text.clone(), cx);
            });
            self.end_input.update(cx, |input: &mut TextInput, cx| {
                input.set_text(draft.end_text.clone(), cx);
            });
            self.replace_line_inputs_with_draft(draft.lines, cx);
        } else {
            self.start_input.update(cx, |input: &mut TextInput, cx| {
                input.set_text(format_seconds_input(start_ms), cx);
            });
            self.end_input.update(cx, |input: &mut TextInput, cx| {
                input.set_text(format_seconds_input(end_ms), cx);
            });
            self.replace_line_inputs(lines, cx);
        }
        self.suppress_input_observers = false;
        self.status = None;
        let preview_start_ms = draft
            .as_ref()
            .and_then(|draft| parse_seconds_input(SharedString::from(draft.start_text.clone())))
            .map(|seconds| normalize_seconds(seconds) * 1000.0)
            .unwrap_or(start_ms);
        self.seek_preview(preview_start_ms);
        self.refresh_dirty_state(cx);
    }

    fn seek_preview(&self, start_ms: f64) {
        let Some(target) = preview_target(start_ms) else {
            return;
        };
        self.player_control.seek_to(target);
        self.player_control.pause();
    }

    fn set_status(
        &mut self,
        text: impl Into<SharedString>,
        is_error: bool,
        cx: &mut Context<Self>,
    ) {
        self.status = Some(StatusMessage {
            text: text.into(),
            is_error,
        });
        cx.notify();
    }

    fn apply_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected_id else {
            self.set_status("Select a subtitle first.", true, cx);
            return;
        };

        let start_seconds = match parse_seconds("Start", self.start_input.read(cx).text()) {
            Ok(value) => value,
            Err(err) => {
                self.set_status(err, true, cx);
                return;
            }
        };

        let end_seconds = match parse_seconds("End", self.end_input.read(cx).text()) {
            Ok(value) => value,
            Err(err) => {
                self.set_status(err, true, cx);
                return;
            }
        };

        let start_ms = normalize_seconds(start_seconds) * 1000.0;
        let end_ms = normalize_seconds(end_seconds) * 1000.0;

        if end_ms < start_ms {
            self.set_status("End time must be >= start time.", true, cx);
            return;
        }

        let lines = self.collect_lines(cx);
        if lines.is_empty() {
            self.set_status("Subtitle text cannot be empty.", true, cx);
            return;
        }

        let edit = SubtitleEdit {
            id,
            start_ms,
            end_ms,
            lines: lines.clone(),
        };
        match self.detection.update_subtitle(edit) {
            Ok(()) => {
                self.update_local_subtitle(id, start_ms, end_ms, lines.clone());
                self.set_selected_snapshot(id, start_ms, end_ms, lines);
                self.dirty = false;
                self.drafts.remove(&id);
                self.set_status("Subtitle updated.", false, cx);
                self.seek_preview(start_ms);
            }
            Err(err) => {
                self.set_status(err, true, cx);
            }
        }
    }

    fn collect_lines_from_draft(&self, draft: &SubtitleDraft) -> Vec<String> {
        let mut raw_lines = Vec::new();
        for line in &draft.lines {
            if line.deleted {
                continue;
            }
            let trimmed = line.text.trim();
            if trimmed.is_empty() {
                continue;
            }
            raw_lines.push(trimmed.to_string());
        }
        normalize_lines(&raw_lines)
    }

    fn build_edit_from_draft(
        &self,
        entry: &EditableSubtitle,
        draft: &SubtitleDraft,
    ) -> Result<SubtitleEdit, SharedString> {
        let label = format_timestamp(entry.start_ms);
        let start_seconds = parse_seconds("Start", SharedString::from(draft.start_text.clone()))
            .map_err(|err| SharedString::from(format!("Subtitle {label}: {err}")))?;
        let end_seconds = parse_seconds("End", SharedString::from(draft.end_text.clone()))
            .map_err(|err| SharedString::from(format!("Subtitle {label}: {err}")))?;

        let start_ms = normalize_seconds(start_seconds) * 1000.0;
        let end_ms = normalize_seconds(end_seconds) * 1000.0;
        if end_ms < start_ms {
            return Err(SharedString::from(format!(
                "Subtitle {label}: End time must be >= start time."
            )));
        }

        let lines = self.collect_lines_from_draft(draft);
        if lines.is_empty() {
            return Err(SharedString::from(format!(
                "Subtitle {label}: Subtitle text cannot be empty."
            )));
        }

        Ok(SubtitleEdit {
            id: entry.id,
            start_ms,
            end_ms,
            lines,
        })
    }

    fn apply_all(&mut self, cx: &mut Context<Self>) {
        let is_dirty = self.calculate_dirty(cx);
        self.sync_current_draft_state(is_dirty, cx);
        if self.drafts.is_empty() {
            return;
        }

        let mut edits = Vec::new();
        for entry in &self.subtitles {
            if let Some(draft) = self.drafts.get(&entry.id) {
                match self.build_edit_from_draft(entry, draft) {
                    Ok(edit) => edits.push(edit),
                    Err(err) => {
                        self.set_status(err, true, cx);
                        return;
                    }
                }
            }
        }

        if edits.is_empty() {
            return;
        }

        let mut applied = 0usize;
        let mut selected_start_ms = None;
        for edit in edits {
            match self.detection.update_subtitle(edit.clone()) {
                Ok(()) => {
                    self.update_local_subtitle(
                        edit.id,
                        edit.start_ms,
                        edit.end_ms,
                        edit.lines.clone(),
                    );
                    if self.selected_id == Some(edit.id) {
                        self.set_selected_snapshot(
                            edit.id,
                            edit.start_ms,
                            edit.end_ms,
                            edit.lines.clone(),
                        );
                        self.dirty = false;
                        selected_start_ms = Some(edit.start_ms);
                    }
                    self.drafts.remove(&edit.id);
                    applied += 1;
                }
                Err(err) => {
                    self.set_status(err, true, cx);
                    return;
                }
            }
        }

        if let Some(start_ms) = selected_start_ms {
            self.seek_preview(start_ms);
        }
        self.set_status(format!("Saved {applied} subtitle(s)."), false, cx);
    }

    fn restore_all(&mut self, cx: &mut Context<Self>) {
        if self.drafts.is_empty() && !self.dirty {
            return;
        }
        self.drafts.clear();
        if let Some(id) = self.selected_id {
            self.load_selected(id, cx);
        } else {
            self.dirty = false;
        }
    }

    fn update_local_subtitle(&mut self, id: u64, start_ms: f64, end_ms: f64, lines: Vec<String>) {
        if let Some(entry) = self.subtitles.iter_mut().find(|entry| entry.id == id) {
            entry.start_ms = start_ms;
            entry.end_ms = end_ms;
            entry.lines = normalize_lines(&lines);
        }
    }

    fn collect_lines(&self, cx: &mut Context<Self>) -> Vec<String> {
        let mut raw_lines = Vec::new();
        for line in &self.line_inputs {
            if line.deleted {
                continue;
            }
            let text = line.input.read(cx).text();
            let trimmed = text.as_ref().trim();
            if trimmed.is_empty() {
                continue;
            }
            raw_lines.push(trimmed.to_string());
        }
        normalize_lines(&raw_lines)
    }

    fn filtered_subtitles(&self) -> Vec<usize> {
        let query = self.search_query.as_ref().trim().to_lowercase();
        let mut filtered: Vec<usize> = if query.is_empty() {
            (0..self.subtitles.len()).collect()
        } else {
            self.subtitles
                .iter()
                .enumerate()
                .filter(|(_, entry)| matches_query(entry, &query))
                .map(|(index, _)| index)
                .collect()
        };
        filtered.sort_by(|a, b| {
            let left = &self.subtitles[*a];
            let right = &self.subtitles[*b];
            left.start_ms
                .partial_cmp(&right.start_ms)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });
        filtered
    }

    fn header_row(
        &self,
        filtered_count: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + 'static {
        let count_color = hsla(0.0, 0.0, 0.6, 1.0);
        let secondary_bg = hsla(0.0, 0.0, 1.0, 0.08);
        let secondary_hover = hsla(0.0, 0.0, 1.0, 0.14);
        let secondary_text = hsla(0.0, 0.0, 0.9, 1.0);
        let disabled_bg = hsla(0.0, 0.0, 0.2, 1.0);
        let disabled_text = hsla(0.0, 0.0, 0.6, 1.0);
        let total = self.subtitles.len();
        let has_pending = self.dirty || !self.drafts.is_empty();
        let apply_all_icon_color = if has_pending {
            secondary_text
        } else {
            disabled_text
        };
        let restore_all_icon_color = if has_pending {
            secondary_text
        } else {
            disabled_text
        };

        let mut restore_all_button = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .h(px(26.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .text_size(px(11.0))
            .child(icon_sm(Icon::RotateCcw, restore_all_icon_color))
            .child("Restore All");

        if has_pending {
            restore_all_button = restore_all_button
                .bg(secondary_bg)
                .text_color(secondary_text)
                .cursor_pointer()
                .hover(move |style| style.bg(secondary_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.restore_all(cx);
                    }),
                );
        } else {
            restore_all_button = restore_all_button.bg(disabled_bg).text_color(disabled_text);
        }

        let mut apply_all_button = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .h(px(26.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .text_size(px(11.0))
            .child(icon_sm(Icon::Check, apply_all_icon_color))
            .child("Apply All");

        if has_pending {
            apply_all_button = apply_all_button
                .bg(secondary_bg)
                .text_color(secondary_text)
                .cursor_pointer()
                .hover(move |style| style.bg(secondary_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.apply_all(cx);
                    }),
                );
        } else {
            apply_all_button = apply_all_button.bg(disabled_bg).text_color(disabled_text);
        }

        let actions = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(restore_all_button)
            .child(apply_all_button);

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(self.search_input.clone()),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(count_color)
                    .child(format!("{filtered_count} / {total}")),
            )
            .child(actions)
    }

    fn status_row(&self) -> impl IntoElement + 'static {
        let default_color = hsla(0.0, 0.0, 0.6, 1.0);
        let (text, color) = if let Some(status) = self.status.as_ref() {
            let color = if status.is_error {
                hsla(0.0, 0.7, 0.6, 1.0)
            } else {
                hsla(0.0, 0.0, 0.8, 1.0)
            };
            (status.text.clone(), color)
        } else {
            ("Ready".into(), default_color)
        };

        div().text_size(px(11.0)).text_color(color).child(text)
    }

    fn subtitle_row(
        &self,
        entry: &EditableSubtitle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + 'static {
        let time_color = hsla(0.0, 0.0, 1.0, 0.6);
        let text_color = hsla(0.0, 0.0, 1.0, 0.9);
        let hover_bg = hsla(0.0, 0.0, 1.0, 0.05);
        let selected_bg = hsla(0.0, 0.0, 1.0, 0.12);
        let border_color = hsla(0.0, 0.0, 1.0, 0.08);
        let dirty_accent = hsla(0.08, 0.85, 0.6, 1.0);
        let dirty_badge_bg = hsla(0.08, 0.75, 0.45, 0.25);
        let dirty_badge_text = hsla(0.08, 0.8, 0.7, 1.0);
        let is_selected = self.selected_id == Some(entry.id);
        let is_dirty = self.drafts.contains_key(&entry.id)
            || (self.selected_id == Some(entry.id) && self.dirty);
        let id = entry.id;
        let start_ms = entry.start_ms;
        let end_ms = entry.end_ms;
        let lines_snapshot = entry.lines.clone();

        let mut lines = div()
            .flex()
            .flex_col()
            .gap(px(LIST_ROW_LINE_GAP))
            .min_w(px(0.0));
        if lines_snapshot.is_empty() {
            lines = lines.child(
                div()
                    .text_size(px(LIST_ROW_TEXT_SIZE))
                    .text_color(text_color)
                    .child(""),
            );
        } else {
            for line in &lines_snapshot {
                lines = lines.child(
                    div()
                        .text_size(px(LIST_ROW_TEXT_SIZE))
                        .text_color(text_color)
                        .child(line.clone()),
                );
            }
        }

        let mut time_row = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(px(LIST_ROW_TIME_HEIGHT))
            .text_size(px(LIST_ROW_TIME_TEXT_SIZE))
            .text_color(time_color)
            .child(format!(
                "{} - {}",
                format_timestamp(start_ms),
                format_timestamp(end_ms)
            ));

        if is_dirty {
            time_row = time_row.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(6.0))
                    .h(px(LIST_ROW_BADGE_HEIGHT))
                    .line_height(px(LIST_ROW_BADGE_HEIGHT))
                    .rounded(px(4.0))
                    .text_size(px(8.0))
                    .text_color(dirty_badge_text)
                    .bg(dirty_badge_bg)
                    .child("Edited"),
            );
        }

        let mut row = div()
            .id(("subtitle-editor-row", entry.id))
            .relative()
            .flex()
            .flex_col()
            .gap(px(LIST_ROW_GAP))
            .w_full()
            .min_w(px(0.0))
            .px(px(10.0))
            .py(px(LIST_ROW_PADDING_Y))
            .border_b(px(1.0))
            .border_color(border_color)
            .cursor_pointer()
            .hover(move |style| style.bg(hover_bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.select_subtitle(id, cx);
                }),
            )
            .child(time_row)
            .child(lines);

        if is_selected {
            row = row.bg(selected_bg);
        }

        if is_dirty {
            row = row.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(6.0))
                    .bottom(px(6.0))
                    .w(px(3.0))
                    .rounded(px(2.0))
                    .bg(dirty_accent),
            );
        }

        row
    }

    fn sync_list_layout(&mut self, row_count: usize) {
        if row_count == self.list_row_heights.len() {
            return;
        }

        self.list_row_heights.clear();
        self.list_row_measured.clear();
        self.list_row_offsets.clear();
        self.list_row_offsets.push(Pixels::ZERO);

        for _ in 0..row_count {
            self.list_row_heights.push(self.list_estimated_row_height);
            self.list_row_measured.push(false);
            if let Some(last) = self.list_row_offsets.last().copied() {
                self.list_row_offsets
                    .push(last + self.list_estimated_row_height);
            }
        }

        self.list_scroll_refresh_pending = true;
        self.list_scrollbar_animation = None;
        self.list_last_scrollbar_metrics = None;
    }

    fn refresh_list_estimated_row_height(&mut self, window: &Window) {
        let time_line_height = self.line_height_for_size(window, px(LIST_ROW_TIME_TEXT_SIZE));
        let body_line_height = self.line_height_for_size(window, px(LIST_ROW_TEXT_SIZE));
        let base = px(LIST_ROW_PADDING_Y * 2.0 + LIST_ROW_GAP);
        let estimated = base + time_line_height + body_line_height * LIST_ESTIMATED_BODY_LINES;

        if (estimated - self.list_estimated_row_height).abs() > px(LIST_HEIGHT_EPS) {
            self.list_estimated_row_height = estimated;
            for (index, measured) in self.list_row_measured.iter().enumerate() {
                if !*measured {
                    self.list_row_heights[index] = estimated;
                }
            }
            self.rebuild_list_row_offsets();
            self.list_scroll_refresh_pending = true;
        }
    }

    fn line_height_for_size(&self, window: &Window, font_size: Pixels) -> Pixels {
        let mut style = window.text_style();
        style.font_size = font_size.into();
        style.line_height_in_pixels(window.rem_size())
    }

    fn rebuild_list_row_offsets(&mut self) {
        self.list_row_offsets.clear();
        self.list_row_offsets.push(Pixels::ZERO);
        for height in &self.list_row_heights {
            if let Some(last) = self.list_row_offsets.last().copied() {
                self.list_row_offsets.push(last + *height);
            }
        }
    }

    fn set_list_row_height(&mut self, index: usize, height: Pixels) {
        if index >= self.list_row_heights.len() {
            return;
        }
        let old_height = self.list_row_heights[index];
        let delta = height - old_height;
        if delta.abs() <= px(LIST_HEIGHT_EPS) {
            return;
        }
        self.list_row_heights[index] = height;
        for offset in self.list_row_offsets.iter_mut().skip(index + 1) {
            *offset += delta;
        }
    }

    fn list_index_for_offset(&self, offset: Pixels) -> usize {
        let mut low = 0usize;
        let mut high = self.list_row_heights.len();
        while low < high {
            let mid = (low + high) / 2;
            if self
                .list_row_offsets
                .get(mid + 1)
                .copied()
                .unwrap_or(Pixels::ZERO)
                <= offset
            {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low
    }

    fn list_end_index_for_offset(&self, offset: Pixels) -> usize {
        let mut low = 0usize;
        let mut high = self.list_row_heights.len();
        while low < high {
            let mid = (low + high) / 2;
            if self
                .list_row_offsets
                .get(mid)
                .copied()
                .unwrap_or(Pixels::ZERO)
                < offset
            {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low
    }

    fn list_visible_range(
        &self,
        scroll_top: Pixels,
        viewport_height: Pixels,
    ) -> (usize, usize, usize) {
        if self.list_row_heights.is_empty() || viewport_height <= Pixels::ZERO {
            let end = self.list_row_heights.len().min(LIST_INITIAL_RENDER_COUNT);
            return (0, 0, end);
        }

        let padding_top = px(LIST_PADDING_TOP);
        let content_top = if scroll_top > padding_top {
            scroll_top - padding_top
        } else {
            Pixels::ZERO
        };
        let content_bottom = content_top + viewport_height;
        let visible_start = self.list_index_for_offset(content_top);
        let overscan = px(LIST_OVERSCAN_PX);
        let render_start = self.list_index_for_offset(if content_top > overscan {
            content_top - overscan
        } else {
            Pixels::ZERO
        });
        let render_end = self.list_end_index_for_offset(content_bottom + overscan);

        (visible_start, render_start, render_end)
    }

    fn update_list_row_heights(
        &mut self,
        render_start: usize,
        visible_start: usize,
        bounds: &[Bounds<Pixels>],
    ) -> bool {
        if self.list_scroll_drag.is_some() {
            return false;
        }

        let mut changed = false;
        let mut scroll_delta = Pixels::ZERO;
        for (offset, bound) in bounds.iter().enumerate() {
            let index = render_start + offset;
            if index >= self.list_row_heights.len() {
                break;
            }
            let new_height = bound.size.height;
            if new_height <= Pixels::ZERO {
                continue;
            }
            let old_height = self.list_row_heights[index];
            let delta = new_height - old_height;
            if delta.abs() > px(LIST_HEIGHT_EPS) {
                if index < visible_start {
                    scroll_delta += delta;
                }
                self.set_list_row_height(index, new_height);
                self.list_row_measured[index] = true;
                changed = true;
            } else if !self.list_row_measured[index] {
                self.list_row_measured[index] = true;
            }
        }

        if scroll_delta != Pixels::ZERO {
            let offset = self.list_scroll_handle.offset();
            self.list_scroll_handle
                .set_offset(point(offset.x, offset.y - scroll_delta));
        }

        changed
    }

    fn schedule_list_scroll_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.list_scroll_refresh_pending {
            return;
        }
        self.list_scroll_refresh_pending = false;
        let handle = cx.entity();
        window.on_next_frame(move |_window, cx| {
            handle.update(cx, |_, cx| {
                cx.notify();
            });
        });
    }

    fn list_scrollbar_metrics(&self) -> Option<ScrollbarMetrics> {
        let bounds = self.list_scroll_handle.bounds();
        let viewport_height = f32::from(bounds.size.height);
        let max_offset = f32::from(self.list_scroll_handle.max_offset().height);
        if viewport_height <= 0.0 || max_offset <= 0.0 {
            return None;
        }

        let content_height = viewport_height + max_offset;
        if content_height <= 0.0 {
            return None;
        }

        let mut thumb_height = viewport_height / content_height * viewport_height;
        let min_thumb_height = 18.0;
        if thumb_height < min_thumb_height {
            thumb_height = min_thumb_height.min(viewport_height);
        }

        let scroll_top = (-f32::from(self.list_scroll_handle.offset().y)).clamp(0.0, max_offset);
        let available = (viewport_height - thumb_height).max(0.0);
        let thumb_top = if max_offset > 0.0 {
            (scroll_top / max_offset) * available
        } else {
            0.0
        };

        Some(ScrollbarMetrics {
            viewport_height,
            max_offset,
            scroll_top,
            thumb_height,
            thumb_top,
        })
    }

    fn locked_scrollbar_metrics(&self, state: ScrollbarDragState) -> Option<ScrollbarMetrics> {
        if state.viewport_height <= 0.0 || state.max_offset <= 0.0 {
            return None;
        }
        let scroll_top =
            (-f32::from(self.list_scroll_handle.offset().y)).clamp(0.0, state.max_offset);
        let available = (state.viewport_height - state.thumb_height).max(0.0);
        let thumb_top = if state.max_offset > 0.0 {
            (scroll_top / state.max_offset) * available
        } else {
            0.0
        };

        Some(ScrollbarMetrics {
            viewport_height: state.viewport_height,
            max_offset: state.max_offset,
            scroll_top,
            thumb_height: state.thumb_height,
            thumb_top,
        })
    }

    fn begin_scroll_drag(&mut self, metrics: ScrollbarMetrics, position: Point<Pixels>) {
        let bounds = self.list_scroll_handle.bounds();
        let local_y = f32::from(position.y - bounds.origin.y);
        self.list_scroll_settle_pending = false;
        self.list_scrollbar_animation = None;
        self.mark_list_scrollbar_interaction();
        self.list_scroll_drag = Some(ScrollbarDragState {
            start_pointer_y: local_y,
            start_scroll_top: metrics.scroll_top,
            viewport_height: metrics.viewport_height,
            max_offset: metrics.max_offset,
            thumb_height: metrics.thumb_height,
        });
    }

    fn update_scroll_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(state) = self.list_scroll_drag else {
            return;
        };
        let bounds = self.list_scroll_handle.bounds();
        let local_y = f32::from(position.y - bounds.origin.y);
        let delta = local_y - state.start_pointer_y;
        let available = (state.viewport_height - state.thumb_height).max(0.0);
        if available <= 0.0 || state.max_offset <= 0.0 {
            return;
        }
        let ratio = state.max_offset / available;
        let next_scroll_top = (state.start_scroll_top + delta * ratio).clamp(0.0, state.max_offset);
        self.list_scroll_handle
            .set_offset(point(px(0.0), px(-next_scroll_top)));
        cx.notify();
    }

    fn end_scroll_drag(&mut self, cx: &mut Context<Self>) {
        if self.list_scroll_drag.take().is_some() {
            self.mark_list_scrollbar_interaction();
            self.list_scroll_settle_pending = true;
            cx.notify();
        }
    }

    fn mark_list_scrollbar_interaction(&mut self) {
        self.list_scrollbar_last_interaction = Some(Instant::now());
    }

    fn list_scrollbar_opacity(&self, now: Instant) -> f32 {
        if self.list_scroll_drag.is_some() || self.list_scrollbar_hovered {
            return 1.0;
        }
        let Some(last) = self.list_scrollbar_last_interaction else {
            return 0.0;
        };
        let delay = Duration::from_millis(SCROLLBAR_FADE_DELAY_MS);
        let fade = Duration::from_millis(SCROLLBAR_FADE_MS);
        let elapsed = now.saturating_duration_since(last);
        if elapsed <= delay {
            return 1.0;
        }
        if fade.as_secs_f32() <= f32::EPSILON {
            return 0.0;
        }
        let t = ((elapsed - delay).as_secs_f32() / fade.as_secs_f32()).min(1.0);
        1.0 - ease_out(t)
    }

    fn scrollbar_overlay(
        &self,
        metrics: ScrollbarMetrics,
        opacity: f32,
        is_dragging: bool,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if opacity <= f32::EPSILON {
            return None;
        }
        let thumb_color = hsla(0.0, 0.0, 1.0, 0.28 * opacity);
        let thumb_hover = hsla(0.0, 0.0, 1.0, 0.55 * opacity);
        let base_color = if is_dragging {
            thumb_hover
        } else {
            thumb_color
        };

        let track = div()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(10.0))
            .block_mouse_except_scroll();

        let thumb = div()
            .id(("subtitle-editor-scrollbar-thumb", cx.entity_id()))
            .absolute()
            .top(px(metrics.thumb_top))
            .left(px(2.0))
            .right(px(2.0))
            .h(px(metrics.thumb_height))
            .rounded(px(4.0))
            .bg(base_color)
            .cursor_default()
            .hover(move |style| style.bg(thumb_hover))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    this.begin_scroll_drag(metrics, event.position);
                    cx.notify();
                }),
            );

        Some(track.child(thumb))
    }

    fn animated_scrollbar_metrics(
        &mut self,
        target: ScrollbarMetrics,
        window: &mut Window,
    ) -> ScrollbarMetrics {
        if !self.list_scroll_settle_pending {
            return target;
        }

        let duration = std::time::Duration::from_millis(SCROLLBAR_ANIMATION_MS);
        let now = std::time::Instant::now();

        let mut animation = if let Some(animation) = self.list_scrollbar_animation {
            if metrics_target_changed(animation.target, target) {
                self.list_scrollbar_animation = None;
                self.list_scroll_settle_pending = false;
                return target;
            }
            animation
        } else {
            let Some(start) = self.list_last_scrollbar_metrics else {
                self.list_scroll_settle_pending = false;
                return target;
            };
            if metrics_close(start, target) {
                self.list_scroll_settle_pending = false;
                return target;
            }
            let animation = ScrollbarAnimation {
                start,
                target,
                started_at: now,
            };
            self.list_scrollbar_animation = Some(animation);
            window.request_animation_frame();
            return start;
        };

        let elapsed = now.saturating_duration_since(animation.started_at);
        let duration_secs = duration.as_secs_f32();
        let mut progress = if duration_secs <= f32::EPSILON {
            1.0
        } else {
            (elapsed.as_secs_f32() / duration_secs).min(1.0)
        };
        progress = ease_out(progress);

        let current = interpolate_metrics(animation.start, animation.target, progress);

        if !metrics_close(animation.target, target) {
            animation = ScrollbarAnimation {
                start: current,
                target,
                started_at: now,
            };
            self.list_scrollbar_animation = Some(animation);
            window.request_animation_frame();
            return current;
        }

        if progress < 1.0 {
            self.list_scrollbar_animation = Some(animation);
            window.request_animation_frame();
            return current;
        }

        self.list_scrollbar_animation = None;
        self.list_scroll_settle_pending = false;
        animation.target
    }

    fn list_panel(
        &mut self,
        filtered: &[usize],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + 'static {
        let empty_color = hsla(0.0, 0.0, 0.6, 1.0);

        self.sync_list_layout(filtered.len());
        self.refresh_list_estimated_row_height(window);
        self.schedule_list_scroll_refresh(window, cx);

        let list_body = if filtered.is_empty() {
            div()
                .flex_1()
                .min_h(px(0.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(120.0))
                        .text_size(px(12.0))
                        .text_color(empty_color)
                        .child("No subtitles match the query."),
                )
                .into_any_element()
        } else {
            let viewport_height = self.list_scroll_handle.bounds().size.height;
            let scroll_top = (-self.list_scroll_handle.offset().y).max(Pixels::ZERO);
            let (visible_start, render_start, render_end) =
                self.list_visible_range(scroll_top, viewport_height);
            let total_height = self
                .list_row_offsets
                .last()
                .copied()
                .unwrap_or(Pixels::ZERO);
            let render_top = self
                .list_row_offsets
                .get(render_start)
                .copied()
                .unwrap_or(Pixels::ZERO);
            let render_bottom = self
                .list_row_offsets
                .get(render_end)
                .copied()
                .unwrap_or(total_height);

            let top_spacer_height = px(LIST_PADDING_TOP) + render_top;
            let bottom_spacer_height = px(LIST_PADDING_BOTTOM) + (total_height - render_bottom);

            let mut rows = div().flex().flex_col().w_full().min_w(px(0.0));
            for index in render_start..render_end {
                if let Some(entry) = filtered
                    .get(index)
                    .and_then(|entry_index| self.subtitles.get(*entry_index))
                {
                    rows = rows.child(self.subtitle_row(entry, cx));
                }
            }

            let handle = cx.entity();
            rows = rows.on_children_prepainted(move |bounds, _window, cx| {
                handle.update(cx, |this, cx| {
                    if this.update_list_row_heights(render_start, visible_start, &bounds) {
                        this.list_scroll_refresh_pending = true;
                        cx.notify();
                    }
                });
            });

            div()
                .flex()
                .flex_col()
                .w_full()
                .min_w(px(0.0))
                .child(div().w_full().h(top_spacer_height))
                .child(rows)
                .child(div().w_full().h(bottom_spacer_height))
                .into_any_element()
        };

        let scroll_area = div()
            .id(("subtitle-editor-list-scroll", cx.entity_id()))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .scrollbar_width(px(8.0))
            .track_scroll(&self.list_scroll_handle)
            .on_scroll_wheel(cx.listener(|this, _event, _window, cx| {
                this.list_scroll_refresh_pending = true;
                this.mark_list_scrollbar_interaction();
                cx.notify();
            }))
            .on_hover(cx.listener(|this, hovered, _window, cx| {
                this.list_scrollbar_hovered = *hovered;
                this.mark_list_scrollbar_interaction();
                cx.notify();
            }))
            .child(list_body);

        let mut container = div()
            .id(("subtitle-editor-list", cx.entity_id()))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .child(scroll_area);

        let metrics = if let Some(state) = self.list_scroll_drag {
            self.locked_scrollbar_metrics(state)
        } else {
            self.list_scrollbar_metrics()
        };

        let now = Instant::now();
        let opacity = self.list_scrollbar_opacity(now);

        if let Some(target) = metrics {
            let metrics = if self.list_scroll_drag.is_some() {
                target
            } else {
                self.animated_scrollbar_metrics(target, window)
            };
            if let Some(scrollbar) =
                self.scrollbar_overlay(metrics, opacity, self.list_scroll_drag.is_some(), cx)
            {
                container = container.child(scrollbar);
            }
            self.list_last_scrollbar_metrics = Some(metrics);
        } else {
            self.list_scrollbar_animation = None;
            self.list_scroll_settle_pending = false;
            self.list_last_scrollbar_metrics = None;
            self.list_scrollbar_last_interaction = None;
        }

        if self.list_scrollbar_last_interaction.is_some()
            && !self.list_scrollbar_hovered
            && self.list_scroll_drag.is_none()
        {
            if opacity > f32::EPSILON {
                window.request_animation_frame();
            } else {
                self.list_scrollbar_last_interaction = None;
            }
        }

        if self.list_scroll_drag.is_some() {
            let handle = cx.entity();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                handle.update(cx, |this, cx| {
                    this.update_scroll_drag(event.position, cx);
                });
                window.refresh();
            });

            let handle = cx.entity();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                if event.button == MouseButton::Left {
                    handle.update(cx, |this, cx| {
                        this.end_scroll_drag(cx);
                    });
                    window.refresh();
                }
            });
        }

        container
    }

    fn preview_panel(&self) -> impl IntoElement + 'static {
        let border_color = hsla(0.0, 0.0, 1.0, 0.12);
        let placeholder_color = hsla(0.0, 0.0, 1.0, 0.55);
        let snapshot = self.player_info.snapshot();
        let aspect_ratio = preview_aspect_ratio(&snapshot);

        let mut preview = div()
            .relative()
            .w_full()
            .map(|mut view| {
                view.style().aspect_ratio = Some(aspect_ratio);
                view
            })
            .border_1()
            .border_color(border_color)
            .bg(rgb(0x111111))
            .child(self.player.clone());

        if self.selected_id.is_none() || !snapshot.has_frame {
            preview = preview.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(placeholder_color)
                    .child("Select a subtitle to preview"),
            );
        }

        preview
    }

    fn editor_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + 'static {
        let label_color = hsla(0.0, 0.0, 0.75, 1.0);
        let hint_color = hsla(0.0, 0.0, 0.55, 1.0);
        let add_bg = hsla(0.0, 0.0, 1.0, 0.08);
        let add_hover = hsla(0.0, 0.0, 1.0, 0.14);
        let add_text = hsla(0.0, 0.0, 0.9, 1.0);
        let line_inputs = self.line_inputs.clone();
        let delete_enabled = !line_inputs.is_empty();
        let start_input = self.start_input.clone();
        let end_input = self.end_input.clone();
        let start_modified = self.time_field_modified(TimeField::Start, cx);
        let end_modified = self.time_field_modified(TimeField::End, cx);

        let time_row = div()
            .flex()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .flex_1()
                    .child(self.time_field_label("Start (s)", start_modified))
                    .child(self.time_input_row(TimeField::Start, start_input, start_modified, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .flex_1()
                    .child(self.time_field_label("End (s)", end_modified))
                    .child(self.time_input_row(TimeField::End, end_input, end_modified, cx)),
            );

        let text_row = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(label_color)
                    .child("Text"),
            )
            .child({
                let mut line_rows = div().flex().flex_col().gap(px(6.0));
                for (index, input) in line_inputs.into_iter().enumerate() {
                    line_rows =
                        line_rows.child(self.line_input_row(index, delete_enabled, input, cx));
                }
                line_rows
            })
            .child(
                div().flex().items_center().child(
                    div().flex_1().min_w(px(0.0)).child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .h(px(30.0))
                            .w_full()
                            .px(px(10.0))
                            .rounded(px(6.0))
                            .bg(add_bg)
                            .text_size(px(11.0))
                            .text_color(add_text)
                            .cursor_pointer()
                            .hover(move |style| style.bg(add_hover))
                            .child(icon_sm(Icon::Plus, add_text))
                            .child("Add line")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.add_line_input(cx);
                                }),
                            ),
                    ),
                ),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(hint_color)
                    .child("Use Add line to insert another subtitle row."),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(time_row)
            .child(text_row)
    }

    fn time_input_row(
        &self,
        field: TimeField,
        input: Entity<TextInput>,
        is_modified: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + 'static {
        let modified_accent = hsla(0.08, 0.85, 0.6, 1.0);
        let modified_bg = hsla(0.08, 0.85, 0.55, 0.12);
        let modified_hover = hsla(0.08, 0.85, 0.35, 0.18);
        let disabled_bg = hsla(0.0, 0.0, 1.0, 0.05);
        let disabled_icon = hsla(0.0, 0.0, 1.0, 0.35);
        let button_bg = hsla(0.0, 0.0, 1.0, 0.06);
        let button_border = rgb(0x2f2f2f);
        let button_hover_border = rgb(0x3a3a3a);
        let modified_border = button_border;
        let modified_hover_border = button_hover_border;

        let mut input_wrapper = div().flex_1().min_w(px(0.0)).relative().child(input);
        if is_modified {
            input_wrapper = input_wrapper.child(
                div()
                    .absolute()
                    .left(px(3.0))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(2.0))
                    .rounded(px(2.0))
                    .bg(modified_accent),
            );
        }

        let mut rollback_button = div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(TRAILING_ICON_BUTTON_HEIGHT))
            .w(px(TIME_INPUT_TRAILING_WIDTH))
            .rounded(px(6.0))
            .bg(button_bg)
            .border_1()
            .border_color(button_border)
            .child(icon_sm(
                Icon::RotateCcw,
                if is_modified {
                    modified_accent
                } else {
                    disabled_icon
                },
            ));

        if is_modified {
            rollback_button = rollback_button
                .cursor_pointer()
                .bg(modified_bg)
                .border_color(modified_border)
                .hover(move |style| style.bg(modified_hover).border_color(modified_hover_border))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.rollback_time_field(field, cx);
                    }),
                );
        } else {
            rollback_button = rollback_button.bg(disabled_bg);
        }

        div()
            .flex()
            .items_center()
            .gap(px(TIME_INPUT_TRAILING_GAP))
            .child(input_wrapper)
            .child(rollback_button)
    }

    fn time_field_label(
        &self,
        label: &'static str,
        is_modified: bool,
    ) -> impl IntoElement + 'static {
        let label_color = hsla(0.0, 0.0, 0.75, 1.0);
        let dirty_badge_bg = hsla(0.08, 0.75, 0.45, 0.25);
        let dirty_badge_text = hsla(0.08, 0.8, 0.7, 1.0);

        let mut row = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(11.0))
            .text_color(label_color)
            .child(label);

        if is_modified {
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(6.0))
                    .h(px(14.0))
                    .line_height(px(14.0))
                    .rounded(px(4.0))
                    .text_size(px(8.0))
                    .text_color(dirty_badge_text)
                    .bg(dirty_badge_bg)
                    .child("Edited"),
            );
        }

        row
    }

    fn line_input_row(
        &self,
        index: usize,
        delete_enabled: bool,
        input_state: LineInputState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + 'static {
        let deleted_bg = hsla(0.0, 0.7, 0.25, 0.16);
        let deleted_text = hsla(0.0, 0.0, 1.0, 0.65);
        let deleted_badge_bg = hsla(0.0, 0.7, 0.45, 0.25);
        let deleted_badge_text = hsla(0.0, 0.7, 0.7, 1.0);
        let modified_accent = hsla(0.08, 0.85, 0.6, 1.0);
        let modified_bg = hsla(0.08, 0.85, 0.55, 0.12);
        let modified_hover = hsla(0.08, 0.85, 0.35, 0.18);
        let disabled_icon = hsla(0.0, 0.0, 1.0, 0.35);
        let new_accent = hsla(0.55, 0.55, 0.65, 1.0);
        let new_bg = hsla(0.55, 0.55, 0.45, 0.12);
        let button_bg = hsla(0.0, 0.0, 1.0, 0.06);
        let button_border = rgb(0x2f2f2f);
        let button_hover_border = rgb(0x3a3a3a);
        let modified_border = button_border;
        let modified_hover_border = button_hover_border;
        let delete_color = if delete_enabled && !input_state.deleted {
            deleted_badge_text
        } else {
            hsla(0.0, 0.0, 1.0, 0.35)
        };
        let delete_hover = deleted_badge_bg;
        let is_new_line = self.line_is_new(index);
        let is_modified = self.line_is_modified(index, &input_state, cx);
        let toggle_icon = Icon::Trash;
        let input = input_state.input.clone();
        let rollback_enabled = is_modified;
        let mut rollback_button = div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(TRAILING_ICON_BUTTON_WIDTH))
            .h(px(TRAILING_ICON_BUTTON_HEIGHT))
            .rounded(px(6.0))
            .bg(button_bg)
            .border_1()
            .border_color(if rollback_enabled {
                modified_border
            } else {
                button_border
            })
            .child(icon_sm(
                Icon::RotateCcw,
                if rollback_enabled {
                    modified_accent
                } else {
                    disabled_icon
                },
            ));

        if rollback_enabled {
            rollback_button = rollback_button
                .cursor_pointer()
                .bg(modified_bg)
                .hover(move |style| style.bg(modified_hover).border_color(modified_hover_border))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.rollback_line(index, cx);
                    }),
                );
        }

        let mut delete_button = div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(TRAILING_ICON_BUTTON_WIDTH))
            .h(px(TRAILING_ICON_BUTTON_HEIGHT))
            .rounded(px(6.0))
            .bg(button_bg)
            .border_1()
            .border_color(button_border)
            .child(icon_sm(toggle_icon, delete_color));

        if delete_enabled && !input_state.deleted {
            delete_button = delete_button
                .cursor_pointer()
                .hover(move |style| style.bg(delete_hover).border_color(button_hover_border))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.toggle_line_deleted(index, cx);
                    }),
                );
        }

        let content = if input_state.deleted {
            let raw_text = input.read(cx).text();
            let text = if raw_text.as_ref().trim().is_empty() {
                LINE_PLACEHOLDER.to_string()
            } else {
                raw_text.to_string()
            };
            let row = div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .h(px(30.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .bg(deleted_bg)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(12.0))
                        .text_color(deleted_text)
                        .text_ellipsis()
                        .child(text),
                )
                .child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(4.0))
                        .text_size(px(9.0))
                        .text_color(deleted_badge_text)
                        .bg(deleted_badge_bg)
                        .child("Deleted"),
                );
            div().flex_1().min_w(px(0.0)).child(row)
        } else {
            let mut wrapper = div().flex_1().min_w(px(0.0)).relative().child(input);
            if is_new_line {
                wrapper = wrapper.rounded(px(6.0)).bg(new_bg).child(
                    div()
                        .absolute()
                        .left(px(3.0))
                        .top(px(4.0))
                        .bottom(px(4.0))
                        .w(px(2.0))
                        .rounded(px(2.0))
                        .bg(new_accent),
                );
            } else if is_modified {
                wrapper = wrapper.rounded(px(6.0)).bg(modified_bg).child(
                    div()
                        .absolute()
                        .left(px(3.0))
                        .top(px(4.0))
                        .bottom(px(4.0))
                        .w(px(2.0))
                        .rounded(px(2.0))
                        .bg(modified_accent),
                );
            }
            wrapper
        };

        let trailing_controls = div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(TRAILING_ICON_BUTTON_GAP))
            .w(px(LINE_INPUT_TRAILING_WIDTH))
            .child(delete_button)
            .child(rollback_button);

        div()
            .flex()
            .items_center()
            .gap(px(TIME_INPUT_TRAILING_GAP))
            .child(content)
            .child(trailing_controls)
    }

    fn action_row(&self, cx: &mut Context<Self>) -> impl IntoElement + 'static {
        let primary_bg = hsla(0.0, 0.0, 0.9, 1.0);
        let primary_hover = hsla(0.0, 0.0, 1.0, 1.0);
        let primary_text = hsla(0.0, 0.0, 0.1, 1.0);
        let secondary_bg = hsla(0.0, 0.0, 1.0, 0.08);
        let secondary_hover = hsla(0.0, 0.0, 1.0, 0.14);
        let secondary_text = hsla(0.0, 0.0, 0.9, 1.0);
        let disabled_bg = hsla(0.0, 0.0, 0.2, 1.0);
        let disabled_text = hsla(0.0, 0.0, 0.6, 1.0);

        let can_apply = self.selected_id.is_some() && self.dirty;
        let can_restore = self.selected_id.is_some() && self.dirty;
        let apply_icon_color = if can_apply {
            primary_text
        } else {
            disabled_text
        };

        let mut apply_button = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .h(px(30.0))
            .px(px(16.0))
            .rounded(px(6.0))
            .text_size(px(12.0))
            .child(icon_sm(Icon::Check, apply_icon_color))
            .child("Apply");

        if can_apply {
            apply_button = apply_button
                .bg(primary_bg)
                .text_color(primary_text)
                .cursor_pointer()
                .hover(move |style| style.bg(primary_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.apply_selected(cx);
                    }),
                );
        } else {
            apply_button = apply_button.bg(disabled_bg).text_color(disabled_text);
        }

        let restore_icon_color = if can_restore {
            secondary_text
        } else {
            disabled_text
        };
        let mut restore_button = div()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .h(px(30.0))
            .px(px(14.0))
            .rounded(px(6.0))
            .text_size(px(12.0))
            .child(icon_sm(Icon::RotateCcw, restore_icon_color))
            .child("Restore");

        if can_restore {
            restore_button = restore_button
                .bg(secondary_bg)
                .text_color(secondary_text)
                .cursor_pointer()
                .hover(move |style| style.bg(secondary_hover))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        if let Some(id) = this.selected_id {
                            this.drafts.remove(&id);
                            this.load_selected(id, cx);
                        }
                    }),
                );
        } else {
            restore_button = restore_button.bg(disabled_bg).text_color(disabled_text);
        }

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(restore_button)
            .child(apply_button)
    }
}

impl Render for SubtitleEditorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_subtitle_listener(window, cx);

        let filtered = self.filtered_subtitles();
        let titlebar = self.titlebar.clone();
        let header_row = self.header_row(filtered.len(), cx);
        let status_row = self.status_row();
        let preview_panel = self.preview_panel();
        let editor_panel = self.editor_panel(cx);
        let action_row = self.action_row(cx);
        let list_panel = self.list_panel(&filtered, window, cx);

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.0))
            .bg(rgb(0x1b1b1b))
            .child(
                div()
                    .flex_none()
                    .border_b(px(1.0))
                    .border_color(rgb(0x2b2b2b))
                    .child(titlebar),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(px(16.0))
                    .px(px(18.0))
                    .py(px(16.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w(px(LIST_MIN_WIDTH))
                            .flex_1()
                            .gap(px(10.0))
                            .child(header_row)
                            .child(status_row)
                            .child(list_panel),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(px(12.0))
                            .child(preview_panel)
                            .child(editor_panel)
                            .child(action_row),
                    ),
            )
    }
}

fn parse_seconds_input(value: SharedString) -> Option<f64> {
    let raw = value.as_ref().trim();
    if raw.is_empty() {
        return None;
    }
    let parsed: f64 = raw.parse().ok()?;
    if !parsed.is_finite() || parsed < 0.0 {
        return None;
    }
    Some(parsed)
}

fn parse_seconds(label: &str, value: SharedString) -> Result<f64, SharedString> {
    let raw = value.as_ref().trim();
    if raw.is_empty() {
        return Err(SharedString::from(format!("{label} time (s) is required.")));
    }
    let parsed: f64 = raw
        .parse()
        .map_err(|_| SharedString::from(format!("{label} time (s) is invalid.")))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(SharedString::from(format!(
            "{label} time (s) must be a finite positive number."
        )));
    }
    Ok(parsed)
}

fn normalize_seconds(seconds: f64) -> f64 {
    (seconds * 100.0).round() / 100.0
}

fn format_seconds_input(ms: f64) -> String {
    if !ms.is_finite() || ms <= 0.0 {
        return "0.00".to_string();
    }
    let seconds = normalize_seconds(ms / 1000.0);
    format!("{seconds:.2}")
}

fn format_seconds_value(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0.00".to_string();
    }
    let value = normalize_seconds(seconds);
    format!("{value:.2}")
}

fn subtitle_editor_title(label: &SharedString) -> SharedString {
    let trimmed = label.as_ref().trim();
    if trimmed.is_empty() {
        "Subtitle Editor".into()
    } else {
        format!("Subtitle Editor - {trimmed}").into()
    }
}

fn matches_query(entry: &EditableSubtitle, query: &str) -> bool {
    if entry.search_text().to_lowercase().contains(query) {
        return true;
    }

    let time_text = format!(
        "{} {} {}",
        format_timestamp(entry.start_ms),
        format_timestamp(entry.end_ms),
        entry.id
    )
    .to_lowercase();
    time_text.contains(query)
}

fn preview_target(start_ms: f64) -> Option<Duration> {
    if !start_ms.is_finite() {
        return None;
    }
    let target_ms = (start_ms + PREVIEW_SEEK_OFFSET_MS).max(0.0);
    Some(Duration::from_secs_f64(target_ms / 1000.0))
}

fn preview_aspect_ratio(snapshot: &VideoPlayerInfoSnapshot) -> f32 {
    let (Some(width), Some(height)) = (snapshot.metadata.width, snapshot.metadata.height) else {
        return DEFAULT_PREVIEW_ASPECT;
    };
    if width == 0 || height == 0 {
        return DEFAULT_PREVIEW_ASPECT;
    }
    let aspect = width as f32 / height as f32;
    if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        DEFAULT_PREVIEW_ASPECT
    }
}

fn normalize_lines(lines: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for line in lines {
        for chunk in line.split("\\n") {
            for part in chunk.split('\n') {
                let trimmed = part.trim();
                if trimmed.is_empty() {
                    continue;
                }
                normalized.push(trimmed.to_string());
            }
        }
    }
    normalized
}

fn format_timestamp(ms: f64) -> String {
    if !ms.is_finite() || ms <= 0.0 {
        return "0:00.000".to_string();
    }
    let total_ms = ms.round().max(0.0) as u64;
    let total_secs = total_ms / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs / 60) % 60;
    let seconds = total_secs % 60;
    let millis = total_ms % 1000;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}.{millis:03}")
    } else {
        format!("{minutes}:{seconds:02}.{millis:03}")
    }
}

fn metrics_close(a: ScrollbarMetrics, b: ScrollbarMetrics) -> bool {
    (a.thumb_top - b.thumb_top).abs() < SCROLLBAR_SETTLE_EPS
        && (a.thumb_height - b.thumb_height).abs() < SCROLLBAR_SETTLE_EPS
}

fn metrics_target_changed(a: ScrollbarMetrics, b: ScrollbarMetrics) -> bool {
    (a.viewport_height - b.viewport_height).abs() > SCROLLBAR_CANCEL_EPS
        || (a.max_offset - b.max_offset).abs() > SCROLLBAR_CANCEL_EPS
}

fn interpolate_metrics(
    start: ScrollbarMetrics,
    target: ScrollbarMetrics,
    t: f32,
) -> ScrollbarMetrics {
    ScrollbarMetrics {
        viewport_height: target.viewport_height,
        max_offset: target.max_offset,
        scroll_top: lerp(start.scroll_top, target.scroll_top, t),
        thumb_height: lerp(start.thumb_height, target.thumb_height, t),
        thumb_top: lerp(start.thumb_top, target.thumb_top, t),
    }
}

fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}
