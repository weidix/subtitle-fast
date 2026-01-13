use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Entity, Render, ScrollHandle, SharedString, Styled, TitlebarOptions,
    Window, WindowBounds, WindowDecorations, WindowHandle, WindowOptions, div, hsla, px, rgb, size,
};

use crate::gui::components::Titlebar;

/// Displays help content and license notices in a dedicated window.
pub struct HelpWindow {
    scroll_handle: ScrollHandle,
    titlebar: Entity<Titlebar>,
}

impl HelpWindow {
    /// Creates a new help window state.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            scroll_handle: ScrollHandle::new(),
            titlebar: cx.new(|_| Titlebar::new("help-titlebar", "Help")),
        }
    }

    /// Opens the help window or returns `None` if it could not be created.
    pub fn open(cx: &mut App) -> Option<WindowHandle<Self>> {
        let bounds = Bounds::centered(None, size(px(820.0), px(640.0)), cx);
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(640.0), px(520.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("subtitle-fast help".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                window_decorations: Some(WindowDecorations::Client),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| HelpWindow::new(cx)),
        ) {
            Ok(handle) => Some(handle),
            Err(err) => {
                eprintln!("failed to open help window: {err}");
                None
            }
        }
    }

    fn section(&self, title: SharedString, lines: Vec<SharedString>) -> impl IntoElement {
        let mut body = div().flex().flex_col().gap(px(6.0));
        for line in lines {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(hsla(0.0, 0.0, 0.78, 1.0))
                    .child(line),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(15.0))
                    .text_color(hsla(0.0, 0.0, 0.92, 1.0))
                    .child(title),
            )
            .child(body)
    }
}

impl Render for HelpWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(self.section(
                "Getting started".into(),
                vec![
                    "Use File -> Add Task (or the Task menu) to load a video.".into(),
                    "Adjust the detection settings in the right sidebar and run the pipeline."
                        .into(),
                    "Results appear in the task list and the detection panel.".into(),
                ],
            ))
            .child(self.section(
                "Task management".into(),
                vec![
                    "Add Task: select a video file to process.".into(),
                    "Remove Task: removes the active task after confirmation.".into(),
                ],
            ))
            .child(self.section(
                "FFmpeg notice".into(),
                vec![
                    "This app can be built with the FFmpeg backend.".into(),
                    "FFmpeg is licensed under the LGPL or GPL depending on configuration.".into(),
                    "If GPL components are enabled, the resulting binaries must follow GPL terms."
                        .into(),
                    "See https://ffmpeg.org/legal.html for details.".into(),
                ],
            ));

        div()
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
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(px(12.0))
                    .px(px(20.0))
                    .py(px(18.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .flex_1()
                            .min_h(px(0.0))
                            .id(("help-scroll", cx.entity_id()))
                            .overflow_y_scroll()
                            .scrollbar_width(px(6.0))
                            .track_scroll(&self.scroll_handle)
                            .child(content),
                    ),
            )
    }
}
