use std::time::{Duration, Instant};

use futures_util::StreamExt;
use gpui::prelude::*;
use gpui::{
    Context, DispatchPhase, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Render, ScrollHandle, Task, Window, div, hsla, point, px,
};

use crate::stage::TimedSubtitle;

use super::{DetectionHandle, SubtitleMessage};
use crate::gui::components::VideoPlayerControlHandle;

#[derive(Clone, Debug)]
struct DetectedSubtitleEntry {
    id: u64,
    start_ms: f64,
    end_ms: f64,
    text: String,
}

impl DetectedSubtitleEntry {
    fn new(id: u64, subtitle: TimedSubtitle) -> Self {
        Self {
            id,
            start_ms: subtitle.start_ms,
            end_ms: subtitle.end_ms,
            text: subtitle.text(),
        }
    }

    fn update(&mut self, subtitle: TimedSubtitle) {
        self.start_ms = subtitle.start_ms;
        self.end_ms = subtitle.end_ms;
        self.text = subtitle.text();
    }
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

pub struct DetectedSubtitlesList {
    handle: DetectionHandle,
    subtitles: Vec<DetectedSubtitleEntry>,
    scroll_handle: ScrollHandle,
    subtitle_task: Option<Task<()>>,
    controls: Option<VideoPlayerControlHandle>,
    scroll_refresh_pending: bool,
    scroll_drag: Option<ScrollbarDragState>,
    scroll_settle_pending: bool,
    scrollbar_animation: Option<ScrollbarAnimation>,
    last_scrollbar_metrics: Option<ScrollbarMetrics>,
}

impl DetectedSubtitlesList {
    pub fn new(handle: DetectionHandle, controls: Option<VideoPlayerControlHandle>) -> Self {
        let snapshot = handle.subtitles_snapshot();
        let mut subtitles = Vec::with_capacity(snapshot.len());
        for subtitle in snapshot {
            subtitles.push(DetectedSubtitleEntry::new(subtitle.id, subtitle));
        }

        Self {
            handle,
            subtitles,
            scroll_handle: ScrollHandle::new(),
            subtitle_task: None,
            controls,
            scroll_refresh_pending: true,
            scroll_drag: None,
            scroll_settle_pending: false,
            scrollbar_animation: None,
            last_scrollbar_metrics: None,
        }
    }

    fn ensure_subtitle_listener(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.subtitle_task.is_some() {
            return;
        }

        let handle = cx.entity();
        let mut subtitle_rx = self.handle.subscribe_subtitles();

        let task = window.spawn(cx, async move |cx| {
            while let Some(message) = subtitle_rx.next().await {
                if cx
                    .update(|_window, cx| {
                        handle.update(cx, |this, cx| {
                            this.apply_message(message);
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

    fn reset_list(&mut self) {
        self.subtitles.clear();
        self.scroll_refresh_pending = true;
        self.scrollbar_animation = None;
        self.last_scrollbar_metrics = None;
    }

    fn push_subtitle(&mut self, subtitle: TimedSubtitle) {
        let entry = DetectedSubtitleEntry::new(subtitle.id, subtitle);
        self.subtitles.push(entry);
        self.scroll_refresh_pending = true;
    }

    fn update_subtitle(&mut self, subtitle: TimedSubtitle) {
        if let Some(existing) = self
            .subtitles
            .iter_mut()
            .find(|entry| entry.id == subtitle.id)
        {
            existing.update(subtitle);
        } else {
            self.push_subtitle(subtitle);
        }
        self.scroll_refresh_pending = true;
    }

    fn apply_message(&mut self, message: SubtitleMessage) {
        match message {
            SubtitleMessage::Reset => self.reset_list(),
            SubtitleMessage::New(subtitle) => self.push_subtitle(subtitle),
            SubtitleMessage::Updated(subtitle) => self.update_subtitle(subtitle),
        }
    }

    fn schedule_scroll_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.scroll_refresh_pending {
            return;
        }
        self.scroll_refresh_pending = false;
        let handle = cx.entity();
        window.on_next_frame(move |_window, cx| {
            handle.update(cx, |_, cx| {
                cx.notify();
            });
        });
    }

    fn scrollbar_metrics(&self) -> Option<ScrollbarMetrics> {
        let bounds = self.scroll_handle.bounds();
        let viewport_height = f32::from(bounds.size.height);
        let max_offset = f32::from(self.scroll_handle.max_offset().height);
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

        let scroll_top = (-f32::from(self.scroll_handle.offset().y)).clamp(0.0, max_offset);
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
        let scroll_top = (-f32::from(self.scroll_handle.offset().y)).clamp(0.0, state.max_offset);
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
        let bounds = self.scroll_handle.bounds();
        let local_y = f32::from(position.y - bounds.origin.y);
        self.scroll_settle_pending = false;
        self.scrollbar_animation = None;
        self.scroll_drag = Some(ScrollbarDragState {
            start_pointer_y: local_y,
            start_scroll_top: metrics.scroll_top,
            viewport_height: metrics.viewport_height,
            max_offset: metrics.max_offset,
            thumb_height: metrics.thumb_height,
        });
    }

    fn update_scroll_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(state) = self.scroll_drag else {
            return;
        };
        let bounds = self.scroll_handle.bounds();
        let local_y = f32::from(position.y - bounds.origin.y);
        let delta = local_y - state.start_pointer_y;
        let available = (state.viewport_height - state.thumb_height).max(0.0);
        if available <= 0.0 || state.max_offset <= 0.0 {
            return;
        }
        let ratio = state.max_offset / available;
        let next_scroll_top = (state.start_scroll_top + delta * ratio).clamp(0.0, state.max_offset);
        self.scroll_handle
            .set_offset(point(px(0.0), px(-next_scroll_top)));
        cx.notify();
    }

    fn end_scroll_drag(&mut self, cx: &mut Context<Self>) {
        if self.scroll_drag.take().is_some() {
            self.scroll_settle_pending = true;
            cx.notify();
        }
    }

    fn subtitle_row(
        &self,
        entry: &DetectedSubtitleEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let time_color = hsla(0.0, 0.0, 1.0, 0.55);
        let text_color = hsla(0.0, 0.0, 1.0, 0.88);
        let divider_color = hsla(0.0, 0.0, 1.0, 0.08);
        let hover_bg = hsla(0.0, 0.0, 1.0, 0.06);
        let time_text = format!(
            "{} - {}",
            format_timestamp(entry.start_ms),
            format_timestamp(entry.end_ms)
        );

        let mut time_row = div()
            .text_size(px(9.0))
            .text_color(time_color)
            .child(time_text);

        if let Some(controls) = self.controls.clone() {
            let start_ms = entry.start_ms;
            time_row = time_row
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _, _, _| {
                        if let Some(target) = seek_target(start_ms) {
                            controls.seek_to(target);
                        }
                    }),
                );
        }

        div()
            .id(("detection-subtitles-row", entry.id))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .w_full()
            .min_w(px(0.0))
            .pt(px(2.0))
            .pb(px(6.0))
            .px(px(2.0))
            .border_b(px(1.0))
            .border_color(divider_color)
            .child(time_row)
            .child(
                div()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(text_color)
                    .child(entry.text.clone()),
            )
    }

    fn empty_placeholder(&self, cx: &Context<Self>) -> impl IntoElement {
        let placeholder_color = hsla(0.0, 0.0, 1.0, 0.4);
        div()
            .id(("detection-subtitles-empty", cx.entity_id()))
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .text_size(px(12.0))
            .text_color(placeholder_color)
            .child("No subtitles detected yet")
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
            .id(("detection-subtitles-scrollbar-thumb", cx.entity_id()))
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
        if !self.scroll_settle_pending {
            return target;
        }

        let duration = Duration::from_millis(SCROLLBAR_ANIMATION_MS);
        let now = Instant::now();

        let mut animation = if let Some(animation) = self.scrollbar_animation {
            if metrics_target_changed(animation.target, target) {
                self.scrollbar_animation = None;
                self.scroll_settle_pending = false;
                return target;
            }
            animation
        } else {
            let Some(start) = self.last_scrollbar_metrics else {
                self.scroll_settle_pending = false;
                return target;
            };
            if metrics_close(start, target) {
                self.scroll_settle_pending = false;
                return target;
            }
            let animation = ScrollbarAnimation {
                start,
                target,
                started_at: now,
            };
            self.scrollbar_animation = Some(animation);
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
            self.scrollbar_animation = Some(animation);
            window.request_animation_frame();
            return current;
        }

        if progress < 1.0 {
            self.scrollbar_animation = Some(animation);
            window.request_animation_frame();
            return current;
        }

        self.scrollbar_animation = None;
        self.scroll_settle_pending = false;
        animation.target
    }
}

impl Render for DetectedSubtitlesList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_subtitle_listener(window, cx);
        self.schedule_scroll_refresh(window, cx);

        let list_body = if self.subtitles.is_empty() {
            div()
                .flex_1()
                .min_h(px(0.0))
                .child(self.empty_placeholder(cx))
        } else {
            let mut rows = div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .w_full()
                .min_w(px(0.0))
                .px(px(2.0))
                .py(px(4.0));

            for entry in &self.subtitles {
                rows = rows.child(self.subtitle_row(entry, cx));
            }
            rows
        };

        let scroll_area = div()
            .id(("detection-subtitles-scroll", cx.entity_id()))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .scrollbar_width(px(8.0))
            .track_scroll(&self.scroll_handle)
            .on_scroll_wheel(cx.listener(|this, _event, _window, cx| {
                this.scroll_refresh_pending = true;
                cx.notify();
            }))
            .child(list_body);

        let mut container = div()
            .id(("detection-subtitles", cx.entity_id()))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .relative()
            .child(scroll_area);

        let metrics = if let Some(state) = self.scroll_drag {
            self.locked_scrollbar_metrics(state)
        } else {
            self.scrollbar_metrics()
        };

        if let Some(target) = metrics {
            let metrics = if self.scroll_drag.is_some() {
                target
            } else {
                self.animated_scrollbar_metrics(target, window)
            };
            if let Some(scrollbar) = self.scrollbar_overlay(metrics, cx) {
                container = container.child(scrollbar);
            }
            self.last_scrollbar_metrics = Some(metrics);
        } else {
            self.scrollbar_animation = None;
            self.scroll_settle_pending = false;
            self.last_scrollbar_metrics = None;
        }

        if self.scroll_drag.is_some() {
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

const SCROLLBAR_ANIMATION_MS: u64 = 180;
const SCROLLBAR_SETTLE_EPS: f32 = 1.0;
const SCROLLBAR_CANCEL_EPS: f32 = 1.0;

fn seek_target(start_ms: f64) -> Option<Duration> {
    if !start_ms.is_finite() {
        return None;
    }
    let target_ms = (start_ms + 100.0).max(0.0);
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
