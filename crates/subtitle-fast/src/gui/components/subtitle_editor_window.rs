use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::Duration;

use futures_util::StreamExt;
use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Entity, MouseButton, Render, SharedString, Subscription, Task, Window,
    WindowBounds, WindowDecorations, WindowOptions, div, hsla, px, rgb, size,
};

use crate::gui::components::config_editor::{InputKind, TextInput};
use crate::gui::components::detection_sidebar::{SubtitleEdit, SubtitleMessage};
use crate::gui::components::video_player::VideoOpenOptions;
use crate::gui::components::{
    DetectionHandle, Titlebar, VideoPlayer, VideoPlayerControlHandle, VideoPlayerInfoHandle,
};
use crate::gui::session::VideoSession;
use crate::subtitle::TimedSubtitle;

const PREVIEW_SEEK_OFFSET_MS: f64 = 100.0;
const LIST_MIN_WIDTH: f32 = 320.0;
const PREVIEW_HEIGHT: f32 = 240.0;

#[derive(Clone, Debug)]
struct EditableSubtitle {
    id: u64,
    start_ms: f64,
    end_ms: f64,
    lines: Vec<String>,
}

impl EditableSubtitle {
    fn from_timed(subtitle: TimedSubtitle) -> Self {
        Self {
            id: subtitle.id,
            start_ms: subtitle.start_ms,
            end_ms: subtitle.end_ms,
            lines: subtitle.lines,
        }
    }

    fn display_text(&self) -> String {
        if self.lines.is_empty() {
            return String::new();
        }
        self.lines.join(" / ")
    }

    fn input_text(&self) -> String {
        if self.lines.is_empty() {
            return String::new();
        }
        self.lines.join("\\n")
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

pub struct SubtitleEditorWindow {
    detection: DetectionHandle,
    video_path: PathBuf,
    subtitles: Vec<EditableSubtitle>,
    subtitle_task: Option<Task<()>>,
    search_input: Entity<TextInput>,
    start_input: Entity<TextInput>,
    end_input: Entity<TextInput>,
    text_input: Entity<TextInput>,
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
}

impl SubtitleEditorWindow {
    pub fn open(session: VideoSession, cx: &mut App) -> Option<gpui::WindowHandle<Self>> {
        let bounds = Bounds::centered(None, size(px(980.0), px(720.0)), cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(820.0), px(600.0))),
                    is_resizable: true,
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("subtitle-fast editor".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    window_decorations: Some(WindowDecorations::Client),
                    ..Default::default()
                },
                move |_, cx| cx.new(|cx| SubtitleEditorWindow::new(session.clone(), cx)),
            )
            .ok()?;

        let _ = handle.update(cx, |_, window, _| {
            window.activate_window();
        });

        Some(handle)
    }

    fn new(session: VideoSession, cx: &mut Context<Self>) -> Self {
        let titlebar = cx.new(|_| Titlebar::new("subtitle-editor-titlebar", "Subtitle Editor"));
        let search_input =
            cx.new(|cx| TextInput::new(cx, "Search subtitles or time...", InputKind::Text));
        let start_input = cx.new(|cx| TextInput::new(cx, "Start ms", InputKind::Float));
        let end_input = cx.new(|cx| TextInput::new(cx, "End ms", InputKind::Float));
        let text_input = cx.new(|cx| {
            TextInput::new(
                cx,
                "Subtitle text (use \\n for line breaks)",
                InputKind::Text,
            )
        });
        let (player, control, info) = VideoPlayer::new();
        let player = cx.new(|_| player);
        let subtitles = session
            .detection
            .subtitles_snapshot()
            .into_iter()
            .map(EditableSubtitle::from_timed)
            .collect::<Vec<_>>();

        let mut window = Self {
            detection: session.detection,
            video_path: session.path,
            subtitles,
            subtitle_task: None,
            search_input,
            start_input,
            end_input,
            text_input,
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

        let text = self.text_input.clone();
        self.subscriptions
            .push(cx.observe(&text, |this, _input, cx| {
                if this.suppress_input_observers {
                    return;
                }
                if this.selected_id.is_some() && !this.dirty {
                    this.dirty = true;
                    cx.notify();
                }
            }));
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
        self.text_input.update(cx, |input: &mut TextInput, cx| {
            input.set_text("", cx);
        });
        self.suppress_input_observers = false;
    }

    fn load_selected(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(entry) = self.subtitles.iter().find(|entry| entry.id == id) else {
            self.selected_id = None;
            self.clear_inputs(cx);
            return;
        };

        self.selected_id = Some(id);
        self.dirty = false;
        self.suppress_input_observers = true;
        self.start_input.update(cx, |input: &mut TextInput, cx| {
            input.set_text(format!("{:.3}", entry.start_ms), cx);
        });
        self.end_input.update(cx, |input: &mut TextInput, cx| {
            input.set_text(format!("{:.3}", entry.end_ms), cx);
        });
        self.text_input.update(cx, |input: &mut TextInput, cx| {
            input.set_text(entry.input_text(), cx);
        });
        self.suppress_input_observers = false;
        self.status = None;
        self.seek_preview(entry.start_ms);
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

        let start_ms = match parse_ms("Start", self.start_input.read(cx).text()) {
            Ok(value) => value,
            Err(err) => {
                self.set_status(err, true, cx);
                return;
            }
        };

        let end_ms = match parse_ms("End", self.end_input.read(cx).text()) {
            Ok(value) => value,
            Err(err) => {
                self.set_status(err, true, cx);
                return;
            }
        };

        if end_ms < start_ms {
            self.set_status("End time must be >= start time.", true, cx);
            return;
        }

        let input_text = self.text_input.read(cx).text();
        let lines = parse_lines(input_text.as_ref());
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
            entry.lines = lines;
        }
    }

    fn filtered_subtitles(&self) -> Vec<&EditableSubtitle> {
        let query = self.search_query.as_ref().trim().to_lowercase();
        let mut filtered: Vec<&EditableSubtitle> = if query.is_empty() {
            self.subtitles.iter().collect()
        } else {
            self.subtitles
                .iter()
                .filter(|entry| matches_query(entry, &query))
                .collect()
        };
        filtered.sort_by(|a, b| {
            a.start_ms
                .partial_cmp(&b.start_ms)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        filtered
    }

    fn header_row(&self, filtered_count: usize) -> impl IntoElement {
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
            .child(self.search_input.clone())
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(count_color)
                    .child(format!("{filtered_count} / {total}")),
            )
    }

    fn status_row(&self) -> impl IntoElement {
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

    fn list_panel(
        &self,
        filtered: &[&EditableSubtitle],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let empty_color = hsla(0.0, 0.0, 0.6, 1.0);
        let time_color = hsla(0.0, 0.0, 1.0, 0.6);
        let text_color = hsla(0.0, 0.0, 1.0, 0.9);
        let hover_bg = hsla(0.0, 0.0, 1.0, 0.05);
        let selected_bg = hsla(0.0, 0.0, 1.0, 0.12);
        let border_color = hsla(0.0, 0.0, 1.0, 0.08);

        let mut list = div().flex().flex_col().gap(px(2.0)).min_w(px(0.0));

        if filtered.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(120.0))
                    .text_size(px(12.0))
                    .text_color(empty_color)
                    .child("No subtitles match the query."),
            );
        } else {
            for entry in filtered {
                let is_selected = self.selected_id == Some(entry.id);
                let id = entry.id;
                let mut row = div()
                    .id(("subtitle-editor-row", entry.id))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .w_full()
                    .min_w(px(0.0))
                    .px(px(10.0))
                    .py(px(8.0))
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
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(time_color)
                            .child(format!(
                                "{} - {}",
                                format_timestamp(entry.start_ms),
                                format_timestamp(entry.end_ms)
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_color)
                            .child(entry.display_text()),
                    );

                if is_selected {
                    row = row.bg(selected_bg);
                }
                list = list.child(row);
            }
        }

        div()
            .id(("subtitle-editor-list", cx.entity_id()))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .child(list)
    }

    fn preview_panel(&self) -> impl IntoElement {
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

    fn editor_panel(&self) -> impl IntoElement {
        let label_color = hsla(0.0, 0.0, 0.75, 1.0);
        let hint_color = hsla(0.0, 0.0, 0.55, 1.0);

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
                            .child("Start (ms)"),
                    )
                    .child(self.start_input.clone()),
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
                            .child("End (ms)"),
                    )
                    .child(self.end_input.clone()),
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
            .child(self.text_input.clone())
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(hint_color)
                    .child("Use \\n to add a line break."),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(time_row)
            .child(text_row)
    }

    fn action_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let primary_bg = hsla(0.0, 0.0, 0.9, 1.0);
        let primary_hover = hsla(0.0, 0.0, 1.0, 1.0);
        let primary_text = hsla(0.0, 0.0, 0.1, 1.0);
        let disabled_bg = hsla(0.0, 0.0, 0.2, 1.0);
        let disabled_text = hsla(0.0, 0.0, 0.6, 1.0);

        let can_apply = self.selected_id.is_some() && self.dirty;

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

        div().flex().items_center().child(apply_button)
    }
}

impl Render for SubtitleEditorWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_subtitle_listener(window, cx);

        let filtered = self.filtered_subtitles();
        let list_panel = self.list_panel(&filtered, cx);

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
                    .child(self.titlebar.clone()),
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
                            .child(self.header_row(filtered.len()))
                            .child(self.status_row())
                            .child(list_panel),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(px(12.0))
                            .child(self.preview_panel())
                            .child(self.editor_panel())
                            .child(self.action_row(cx)),
                    ),
            )
    }
}

fn parse_ms(label: &str, value: SharedString) -> Result<f64, SharedString> {
    let raw = value.as_ref().trim();
    if raw.is_empty() {
        return Err(SharedString::from(format!("{label} time is required.")));
    }
    let parsed: f64 = raw
        .parse()
        .map_err(|_| SharedString::from(format!("{label} time is invalid.")))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(SharedString::from(format!(
            "{label} time must be a finite positive number."
        )));
    }
    Ok(parsed)
}

fn parse_lines(input: &str) -> Vec<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let parts = if trimmed.contains("\\n") {
        trimmed.split("\\n").collect::<Vec<_>>()
    } else {
        vec![trimmed]
    };
    parts
        .into_iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
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
