use gpui::prelude::*;
use gpui::{
    App, Bounds, Context, Entity, Hsla, Render, ScrollHandle, SharedString, Styled,
    TitlebarOptions, Window, WindowBounds, WindowDecorations, WindowHandle, WindowOptions, div,
    hsla, px, rgb, size,
};

use crate::gui::components::Titlebar;
use crate::gui::icons::{Icon, icon_md, logo_full_color};

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
        let bounds = Bounds::centered(None, size(px(820.0), px(560.0)), cx);
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(640.0), px(480.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("subtitle-fast help".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                window_decorations: Some(WindowDecorations::Client),
                ..Default::default()
            },
            |_, cx| cx.new(HelpWindow::new),
        ) {
            Ok(handle) => Some(handle),
            Err(err) => {
                eprintln!("failed to open help window: {err}");
                None
            }
        }
    }

    fn hero(&self) -> impl IntoElement {
        let accent = hsla(0.0, 0.0, 1.0, 1.0);
        let accent_bg = hsla(0.0, 0.0, 1.0, 0.08);

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(16.0))
            .rounded(px(14.0))
            .bg(rgb(0x151515))
            .border_1()
            .border_color(rgb(0x262626))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(logo_full_color().w(px(72.0)).h(px(72.0)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .text_size(px(18.0))
                                    .text_color(hsla(0.0, 0.0, 0.95, 1.0))
                                    .child("subtitle-fast help"),
                            )
                            .child(div().text_size(px(12.0)).text_color(hsla(0.0, 0.0, 0.7, 1.0)).child(
                                "From video import to usable subtitles, in a few clear steps.",
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(px(28.0))
                            .h(px(28.0))
                            .rounded(px(8.0))
                            .bg(accent_bg)
                            .border_1()
                            .border_color(accent)
                            .child(icon_md(Icon::Info, accent)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(hsla(0.0, 0.0, 0.72, 1.0))
                            .child("Settings save when a field loses focus or when Save is used."),
                    ),
            )
    }

    fn section_card(
        &self,
        icon: Icon,
        title: SharedString,
        subtitle: SharedString,
        lines: Vec<SharedString>,
        accent: Hsla,
        accent_bg: Hsla,
    ) -> impl IntoElement {
        let card_bg = rgb(0x151515);
        let card_border = rgb(0x262626);
        let title_color = hsla(0.0, 0.0, 0.92, 1.0);
        let subtitle_color = hsla(0.0, 0.0, 0.68, 1.0);
        let body_color = hsla(0.0, 0.0, 0.78, 1.0);
        let mut body = div().flex().flex_col().gap(px(6.0));
        for line in lines {
            body = body.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(10.0))
                    .child(
                        div()
                            .mt(px(6.0))
                            .w(px(4.0))
                            .h(px(4.0))
                            .rounded(px(2.0))
                            .bg(accent),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .text_color(body_color)
                            .child(line),
                    ),
            );
        }

        let header = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(30.0))
                    .h(px(30.0))
                    .rounded(px(9.0))
                    .bg(accent_bg)
                    .border_1()
                    .border_color(accent)
                    .child(icon_md(icon, accent)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(title_color)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(subtitle_color)
                            .child(subtitle),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(14.0))
            .rounded(px(12.0))
            .bg(card_bg)
            .border_1()
            .border_color(card_border)
            .child(header)
            .child(body)
    }

    fn notice_card(&self, title: SharedString, lines: Vec<SharedString>) -> impl IntoElement {
        let card_bg = rgb(0x141414);
        let card_border = rgb(0x262626);
        let title_color = hsla(0.0, 0.0, 0.9, 1.0);
        let subtitle_color = hsla(0.0, 0.0, 0.72, 1.0);
        let icon_color = hsla(0.0, 0.0, 0.75, 1.0);
        let mut body = div().flex().flex_col().gap(px(6.0));
        for line in lines {
            body = body.child(
                div()
                    .text_size(px(12.0))
                    .text_color(subtitle_color)
                    .child(line),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .p(px(14.0))
            .rounded(px(12.0))
            .bg(card_bg)
            .border_1()
            .border_color(card_border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(icon_md(Icon::Info, icon_color))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(title_color)
                            .child(title),
                    ),
            )
            .child(body)
    }
}

impl Render for HelpWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = hsla(0.0, 0.0, 1.0, 1.0);
        let accent_bg = hsla(0.0, 0.0, 1.0, 0.08);
        let content = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(self.hero())
            .child(self.section_card(
                Icon::PlaySquare,
                "Quick start".into(),
                "A short path to your first subtitle run.".into(),
                vec![
                    "Add a task from the File menu to pick a video.".into(),
                    "Tune detection thresholds in the right sidebar.".into(),
                    "Run the pipeline and review the candidates.".into(),
                ],
                accent,
                accent_bg,
            ))
            .child(self.section_card(
                Icon::SlidersHorizontal,
                "Tuning cues".into(),
                "Small changes can make a big difference.".into(),
                vec![
                    "Tighten the ROI to speed up detection and reduce noise.".into(),
                    "Lower target or delta for faint subtitles; raise them for grainy footage."
                        .into(),
                    "Switch comparators or decoder backends to balance speed and quality.".into(),
                ],
                accent,
                accent_bg,
            ))
            .child(self.section_card(
                Icon::GalleryThumbnails,
                "Review and refine".into(),
                "Iterate quickly before exporting.".into(),
                vec![
                    "Use the task list and detection panel to inspect results.".into(),
                    "Adjust settings and rerun to improve difficult scenes.".into(),
                    "Save settings or reload to compare different passes.".into(),
                ],
                accent,
                accent_bg,
            ))
            .child(self.notice_card(
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
