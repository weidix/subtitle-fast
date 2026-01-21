use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::Duration;

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
use crate::gui::components::{
    DetectionHandle, Titlebar, VideoPlayer, VideoPlayerControlHandle, VideoPlayerInfoHandle,
};
use crate::gui::icons::{Icon, icon_sm};
use crate::gui::session::VideoSession;
use crate::subtitle::TimedSubtitle;

const PREVIEW_SEEK_OFFSET_MS: f64 = 100.0;
const LIST_MIN_WIDTH: f32 = 320.0;
const PREVIEW_HEIGHT: f32 = 240.0;
const LIST_ROW_PADDING_Y: f32 = 8.0;
const LIST_ROW_GAP: f32 = 4.0;
const LIST_ROW_LINE_GAP: f32 = 2.0;
const LIST_ROW_TIME_TEXT_SIZE: f32 = 10.0;
const LIST_ROW_TEXT_SIZE: f32 = 12.0;
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
const LINE_PLACEHOLDER: &str = "Subtitle line";

#[derive(Clone, Debug)]
struct EditableSubtitle {
    id: u64,
    start_ms: f64,
    end_ms: f64,
    lines: Vec<String>,
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
    started_at: std::time::Instant,
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
        };

        window.register_input_observers(cx);
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
                if this.selected_id.is_some() && !this.dirty {
                    this.dirty = true;
                    cx.notify();
                }
            }));

        let end = self.end_input.clone();
        self.subscriptions
            .push(cx.observe(&end, |this, _input, cx| {
                if this.suppress_input_observers {
                    return;
                }
                if this.selected_id.is_some() && !this.dirty {
                    this.dirty = true;
                    cx.notify();
                }
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
                    if this.selected_id.is_some() && !this.dirty {
                        this.dirty = true;
                        cx.notify();
                    }
                }));
        }
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

    fn add_line_input(&mut self, cx: &mut Context<Self>) {
        let input_state = Self::build_line_input(cx);
        self.line_inputs.push(input_state);
        self.register_line_observers(cx);
        if self.selected_id.is_some() && !self.dirty {
            self.dirty = true;
        }
        cx.notify();
    }

    fn toggle_line_deleted(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(line) = self.line_inputs.get_mut(index) else {
            return;
        };
        line.deleted = !line.deleted;
        if self.selected_id.is_some() && !self.dirty {
            self.dirty = true;
        }
        cx.notify();
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
            self.clear_inputs(cx);
            return;
        };

        let (start_ms, end_ms, lines) = {
            let entry = &self.subtitles[entry_index];
            (entry.start_ms, entry.end_ms, entry.lines.clone())
        };

        self.selected_id = Some(id);
        self.dirty = false;
        self.suppress_input_observers = true;
        self.start_input.update(cx, |input: &mut TextInput, cx| {
            input.set_text(format_seconds_input(start_ms), cx);
        });
        self.end_input.update(cx, |input: &mut TextInput, cx| {
            input.set_text(format_seconds_input(end_ms), cx);
        });
        self.replace_line_inputs(lines, cx);
        self.suppress_input_observers = false;
        self.status = None;
        self.seek_preview(start_ms);
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
                self.update_local_subtitle(id, start_ms, end_ms, lines);
                self.dirty = false;
                self.set_status("Subtitle updated.", false, cx);
                self.seek_preview(start_ms);
            }
            Err(err) => {
                self.set_status(err, true, cx);
            }
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

    fn header_row(&self, filtered_count: usize) -> impl IntoElement + 'static {
        let label_color = hsla(0.0, 0.0, 0.8, 1.0);
        let count_color = hsla(0.0, 0.0, 0.6, 1.0);
        let total = self.subtitles.len();
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(label_color)
                    .child("Search"),
            )
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
        let is_dirty = self.selected_id == Some(entry.id) && self.dirty;
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
                    .px(px(6.0))
                    .py(px(1.0))
                    .rounded(px(4.0))
                    .text_size(px(9.0))
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
                    this.load_selected(id, cx);
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
            self.list_scroll_settle_pending = true;
            cx.notify();
        }
    }

    fn scrollbar_overlay(&self, metrics: ScrollbarMetrics, cx: &mut Context<Self>) -> Option<Div> {
        let thumb_color = hsla(0.0, 0.0, 1.0, 0.55);
        let thumb_hover = hsla(0.0, 0.0, 1.0, 0.72);

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
            .bg(thumb_color)
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
                if let Some(entry_index) = filtered.get(index) {
                    if let Some(entry) = self.subtitles.get(*entry_index) {
                        rows = rows.child(self.subtitle_row(entry, cx));
                    }
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

        if let Some(target) = metrics {
            let metrics = if self.list_scroll_drag.is_some() {
                target
            } else {
                self.animated_scrollbar_metrics(target, window)
            };
            if let Some(scrollbar) = self.scrollbar_overlay(metrics, cx) {
                container = container.child(scrollbar);
            }
            self.list_last_scrollbar_metrics = Some(metrics);
        } else {
            self.list_scrollbar_animation = None;
            self.list_scroll_settle_pending = false;
            self.list_last_scrollbar_metrics = None;
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

        let mut preview = div()
            .relative()
            .h(px(PREVIEW_HEIGHT))
            .w_full()
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

        let time_row = div()
            .flex()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .flex_1()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(label_color)
                            .child("Start (s)"),
                    )
                    .child(start_input),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .flex_1()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(label_color)
                            .child("End (s)"),
                    )
                    .child(end_input),
            );

        let text_row = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
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
                        line_rows.child(Self::line_input_row(index, delete_enabled, input, cx));
                }
                line_rows
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(28.0))
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

    fn line_input_row(
        index: usize,
        delete_enabled: bool,
        input_state: LineInputState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + 'static {
        let deleted_bg = hsla(0.0, 0.7, 0.25, 0.16);
        let deleted_text = hsla(0.0, 0.0, 1.0, 0.65);
        let deleted_badge_bg = hsla(0.0, 0.7, 0.45, 0.25);
        let deleted_badge_text = hsla(0.0, 0.7, 0.7, 1.0);
        let restore_color = hsla(0.12, 0.7, 0.65, 1.0);
        let restore_hover = hsla(0.12, 0.7, 0.35, 0.18);
        let delete_color = if delete_enabled {
            if input_state.deleted {
                restore_color
            } else {
                hsla(0.0, 0.0, 1.0, 0.75)
            }
        } else {
            hsla(0.0, 0.0, 1.0, 0.35)
        };
        let delete_hover = if input_state.deleted {
            restore_hover
        } else {
            hsla(0.0, 0.0, 1.0, 0.12)
        };
        let toggle_icon = if input_state.deleted {
            Icon::RotateCcw
        } else {
            Icon::Trash
        };
        let input = input_state.input.clone();
        let mut delete_button = div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(28.0))
            .h(px(28.0))
            .rounded(px(6.0))
            .child(icon_sm(toggle_icon, delete_color));

        if delete_enabled {
            delete_button = delete_button
                .cursor_pointer()
                .hover(move |style| style.bg(delete_hover))
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
            div().flex_1().min_w(px(0.0)).child(
                div()
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
                    ),
            )
        } else {
            div().flex_1().min_w(px(0.0)).child(input)
        };

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(content)
            .child(delete_button)
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

        let mut apply_button = div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(30.0))
            .px(px(16.0))
            .rounded(px(6.0))
            .text_size(px(12.0))
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
        let header_row = self.header_row(filtered.len());
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
