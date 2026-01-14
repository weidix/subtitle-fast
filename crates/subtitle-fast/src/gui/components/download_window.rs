use std::sync::Arc;

use futures_channel::mpsc::UnboundedReceiver;
use futures_util::StreamExt;
use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Entity, FontWeight, MouseButton, Render, SharedString, Task,
    TitlebarOptions, Window, WindowBounds, WindowDecorations, WindowHandle, WindowOptions, div,
    hsla, px, relative, rgb, size,
};

use crate::gui::components::Titlebar;
use crate::model::ModelDownloadEvent;

type DownloadWindowCallback = Arc<dyn Fn(&mut Window, &mut App) + Send + Sync>;

#[derive(Clone, Debug)]
struct DownloadProgress {
    file_label: SharedString,
    file_index: usize,
    file_count: usize,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
}

impl Default for DownloadProgress {
    fn default() -> Self {
        Self {
            file_label: SharedString::from(""),
            file_index: 0,
            file_count: 0,
            downloaded_bytes: 0,
            total_bytes: None,
        }
    }
}

#[derive(Clone, Debug)]
enum DownloadPhase {
    Downloading,
    Failed(SharedString),
}

/// Window displaying ORT model download progress.
pub struct DownloadWindow {
    titlebar: Entity<Titlebar>,
    progress: DownloadProgress,
    phase: DownloadPhase,
    on_continue: Option<DownloadWindowCallback>,
    on_exit: Option<DownloadWindowCallback>,
    progress_task: Option<Task<()>>,
}

impl DownloadWindow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            titlebar: cx.new(|_| Titlebar::new("download-titlebar", "Downloading")),
            progress: DownloadProgress::default(),
            phase: DownloadPhase::Downloading,
            on_continue: None,
            on_exit: None,
            progress_task: None,
        }
    }

    pub fn open(cx: &mut App) -> Option<WindowHandle<Self>> {
        let bounds = Bounds::centered(None, size(px(420.0), px(220.0)), cx);
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(420.0), px(220.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("subtitle-fast download".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                window_decorations: Some(WindowDecorations::Client),
                is_resizable: false,
                ..Default::default()
            },
            |_, cx| cx.new(DownloadWindow::new),
        ) {
            Ok(handle) => {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
                cx.activate(true);
                Some(handle)
            }
            Err(err) => {
                eprintln!("failed to open download window: {err}");
                None
            }
        }
    }

    pub fn bind_progress(
        &mut self,
        mut progress_rx: UnboundedReceiver<ModelDownloadEvent>,
        handle: WindowHandle<Self>,
        on_continue: DownloadWindowCallback,
        on_exit: DownloadWindowCallback,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.on_continue = Some(on_continue);
        self.on_exit = Some(on_exit);

        if self.progress_task.is_some() {
            return;
        }

        let task = window.spawn(cx, async move |cx| {
            while let Some(event) = progress_rx.next().await {
                if handle
                    .update(cx, |this, window, cx| this.apply_event(event, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.progress_task = Some(task);
    }

    fn apply_event(
        &mut self,
        event: ModelDownloadEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ModelDownloadEvent::Started {
                file_label,
                file_index,
                file_count,
                total_bytes,
            } => {
                self.progress = DownloadProgress {
                    file_label: file_label.into(),
                    file_index,
                    file_count,
                    downloaded_bytes: 0,
                    total_bytes,
                };
                self.phase = DownloadPhase::Downloading;
            }
            ModelDownloadEvent::Progress {
                downloaded_bytes,
                total_bytes,
            } => {
                self.progress.downloaded_bytes = downloaded_bytes;
                if total_bytes.is_some() {
                    self.progress.total_bytes = total_bytes;
                }
            }
            ModelDownloadEvent::Finished { file_label } => {
                self.progress.file_label = file_label.into();
                if let Some(total) = self.progress.total_bytes {
                    self.progress.downloaded_bytes = total;
                }
            }
            ModelDownloadEvent::Completed => {
                if let Some(on_continue) = self.on_continue.as_ref() {
                    on_continue(window, cx);
                }
            }
            ModelDownloadEvent::Failed { message } => {
                self.phase = DownloadPhase::Failed(message.into());
            }
        }
        cx.notify();
    }

    fn progress_ratio(&self) -> f32 {
        match self.progress.total_bytes {
            Some(total) if total > 0 => {
                (self.progress.downloaded_bytes as f32 / total as f32).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }

    fn format_bytes(bytes: u64) -> String {
        const KB: f64 = 1024.0;
        const MB: f64 = 1024.0 * 1024.0;
        const GB: f64 = 1024.0 * 1024.0 * 1024.0;
        let value = bytes as f64;
        if value >= GB {
            format!("{:.2} GB", value / GB)
        } else if value >= MB {
            format!("{:.2} MB", value / MB)
        } else if value >= KB {
            format!("{:.1} KB", value / KB)
        } else {
            format!("{bytes} B")
        }
    }
}

impl Render for DownloadWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel_bg = rgb(0x121212);
        let panel_border = rgb(0x262626);
        let text_primary = hsla(0.0, 0.0, 0.92, 1.0);
        let text_secondary = hsla(0.0, 0.0, 0.72, 1.0);
        let accent = hsla(28.0, 0.82, 0.6, 1.0);

        let header = div()
            .text_size(px(14.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(text_primary)
            .child("Downloading OCR assets");

        let body = match &self.phase {
            DownloadPhase::Downloading => {
                let file_line = if self.progress.file_label.is_empty() {
                    "Preparing download...".to_string()
                } else if self.progress.file_count > 0 {
                    format!(
                        "Downloading {} ({}/{})",
                        self.progress.file_label,
                        self.progress.file_index,
                        self.progress.file_count
                    )
                } else {
                    format!("Downloading {}", self.progress.file_label)
                };

                let ratio = self.progress_ratio();
                let total = self.progress.total_bytes.map(Self::format_bytes);
                let downloaded = Self::format_bytes(self.progress.downloaded_bytes);
                let bytes_line = match total {
                    Some(total) => format!("{downloaded} / {total}"),
                    None => downloaded,
                };

                let progress_fill = div()
                    .h(px(8.0))
                    .rounded(px(6.0))
                    .bg(accent)
                    .w(relative(ratio));

                let progress_bar = div()
                    .h(px(8.0))
                    .rounded(px(6.0))
                    .bg(hsla(0.0, 0.0, 1.0, 0.12))
                    .border_1()
                    .border_color(hsla(0.0, 0.0, 1.0, 0.16))
                    .child(progress_fill);

                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_secondary)
                            .child(file_line),
                    )
                    .child(progress_bar)
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_secondary)
                            .child(bytes_line),
                    )
            }
            DownloadPhase::Failed(message) => {
                let on_continue = self.on_continue.clone();
                let on_exit = self.on_exit.clone();

                let continue_button = div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(30.0))
                    .px(px(14.0))
                    .rounded(px(6.0))
                    .bg(accent)
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(hsla(0.0, 0.0, 0.12, 1.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hsla(28.0, 0.85, 0.66, 1.0)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _event, window, cx| {
                            if let Some(callback) = on_continue.as_ref() {
                                callback(window, cx);
                            }
                        }),
                    )
                    .child("Open App");

                let exit_button = div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(30.0))
                    .px(px(14.0))
                    .rounded(px(6.0))
                    .bg(hsla(0.0, 0.0, 0.2, 1.0))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(hsla(0.0, 0.0, 0.88, 1.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hsla(0.0, 0.0, 0.26, 1.0)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_, _event, window, cx| {
                            if let Some(callback) = on_exit.as_ref() {
                                callback(window, cx);
                            }
                        }),
                    )
                    .child("Exit");

                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_secondary)
                            .child("Model download failed."),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_secondary)
                            .child(message.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(continue_button)
                            .child(exit_button),
                    )
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(panel_bg)
            .border_1()
            .border_color(panel_border)
            .child(self.titlebar.clone())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .p(px(16.0))
                    .child(header)
                    .child(body),
            )
    }
}
