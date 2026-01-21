use std::time::{Duration, Instant};

use futures_util::StreamExt;
use gpui::prelude::*;
use gpui::{
    Bounds, Context, DispatchPhase, Div, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, Render, ScrollHandle, Task, Window, div, hsla, point, px,
};

use crate::stage::TimedSubtitle;

use super::{DetectionHandle, SubtitleMessage};
use crate::gui::components::VideoPlayerControlHandle;

#[derive(Clone, Debug)]
struct DetectedSubtitleEntry {
    id: u64,
    start_ms: f64,
    end_ms: f64,
    lines: Vec<String>,
}

impl DetectedSubtitleEntry {
    fn new(id: u64, subtitle: TimedSubtitle) -> Self {
        Self {
            id,
            start_ms: subtitle.start_ms,
            end_ms: subtitle.end_ms,
            lines: normalize_lines(&subtitle.lines),
        }
    }

    fn update(&mut self, subtitle: TimedSubtitle) {
        self.start_ms = subtitle.start_ms;
        self.end_ms = subtitle.end_ms;
        self.lines = normalize_lines(&subtitle.lines);
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
    row_heights: Vec<Pixels>,
    row_offsets: Vec<Pixels>,
    row_measured: Vec<bool>,
    estimated_row_height: Pixels,
    scroll_handle: ScrollHandle,
    subtitle_task: Option<Task<()>>,
    controls: Option<VideoPlayerControlHandle>,
    scroll_refresh_pending: bool,
    scroll_drag: Option<ScrollbarDragState>,
    scroll_settle_pending: bool,
    scrollbar_animation: Option<ScrollbarAnimation>,
    last_scrollbar_metrics: Option<ScrollbarMetrics>,
    scrollbar_hovered: bool,
    scrollbar_last_interaction: Option<Instant>,
}

impl DetectedSubtitlesList {
    pub fn new(handle: DetectionHandle, controls: Option<VideoPlayerControlHandle>) -> Self {
        let snapshot = handle.subtitles_snapshot();
        let mut subtitles = Vec::with_capacity(snapshot.len());
        for subtitle in snapshot {
            subtitles.push(DetectedSubtitleEntry::new(subtitle.id, subtitle));
        }

        let estimated_row_height = px(DEFAULT_ESTIMATED_ROW_HEIGHT);
        let mut row_heights = Vec::with_capacity(subtitles.len());
        let mut row_offsets = Vec::with_capacity(subtitles.len() + 1);
        let mut row_measured = Vec::with_capacity(subtitles.len());
        row_offsets.push(Pixels::ZERO);
        for _ in 0..subtitles.len() {
            row_heights.push(estimated_row_height);
            row_measured.push(false);
            if let Some(last) = row_offsets.last().copied() {
                row_offsets.push(last + estimated_row_height);
            }
        }

        Self {
            handle,
            subtitles,
            row_heights,
            row_offsets,
            row_measured,
            estimated_row_height,
            scroll_handle: ScrollHandle::new(),
            subtitle_task: None,
            controls,
            scroll_refresh_pending: true,
            scroll_drag: None,
            scroll_settle_pending: false,
            scrollbar_animation: None,
            last_scrollbar_metrics: None,
            scrollbar_hovered: false,
            scrollbar_last_interaction: None,
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
        self.row_heights.clear();
        self.row_measured.clear();
        self.row_offsets.clear();
        self.row_offsets.push(Pixels::ZERO);
        self.scroll_refresh_pending = true;
        self.scrollbar_animation = None;
        self.last_scrollbar_metrics = None;
    }

    fn push_subtitle(&mut self, subtitle: TimedSubtitle) {
        let entry = DetectedSubtitleEntry::new(subtitle.id, subtitle);
        self.subtitles.push(entry);
        self.push_row_height(self.estimated_row_height);
        self.scroll_refresh_pending = true;
    }

    fn update_subtitle(&mut self, subtitle: TimedSubtitle) {
        if let Some(index) = self
            .subtitles
            .iter()
            .position(|entry| entry.id == subtitle.id)
        {
            if let Some(existing) = self.subtitles.get_mut(index) {
                existing.update(subtitle);
            }
            self.mark_row_unmeasured(index);
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

    fn refresh_estimated_row_height(&mut self, window: &Window) {
        let time_line_height = self.line_height_for_size(window, px(TIME_TEXT_SIZE));
        let body_line_height = self.line_height_for_size(window, px(BODY_TEXT_SIZE));
        let base = px(ROW_TOP_PADDING + ROW_BOTTOM_PADDING + ROW_ITEM_SPACING + ROW_INTERNAL_GAP);
        let estimated = base + time_line_height + body_line_height * ESTIMATED_BODY_LINES;

        if (estimated - self.estimated_row_height).abs() > px(HEIGHT_EPS) {
            self.estimated_row_height = estimated;
            for (index, measured) in self.row_measured.iter().enumerate() {
                if !*measured {
                    self.row_heights[index] = estimated;
                }
            }
            self.rebuild_row_offsets();
            self.scroll_refresh_pending = true;
        }
    }

    fn line_height_for_size(&self, window: &Window, font_size: Pixels) -> Pixels {
        let mut style = window.text_style();
        style.font_size = font_size.into();
        style.line_height_in_pixels(window.rem_size())
    }

    fn rebuild_row_offsets(&mut self) {
        self.row_offsets.clear();
        self.row_offsets.push(Pixels::ZERO);
        for height in &self.row_heights {
            if let Some(last) = self.row_offsets.last().copied() {
                self.row_offsets.push(last + *height);
            }
        }
    }

    fn push_row_height(&mut self, height: Pixels) {
        self.row_heights.push(height);
        self.row_measured.push(false);
        if let Some(last) = self.row_offsets.last().copied() {
            self.row_offsets.push(last + height);
        } else {
            self.row_offsets.push(Pixels::ZERO);
            self.row_offsets.push(height);
        }
    }

    fn set_row_height(&mut self, index: usize, height: Pixels) {
        if index >= self.row_heights.len() {
            return;
        }
        let old_height = self.row_heights[index];
        let delta = height - old_height;
        if delta.abs() <= px(HEIGHT_EPS) {
            return;
        }
        self.row_heights[index] = height;
        for offset in self.row_offsets.iter_mut().skip(index + 1) {
            *offset += delta;
        }
    }

    fn mark_row_unmeasured(&mut self, index: usize) {
        if index >= self.row_heights.len() {
            return;
        }
        self.row_measured[index] = false;
        self.set_row_height(index, self.estimated_row_height);
    }

    fn index_for_offset(&self, offset: Pixels) -> usize {
        let mut low = 0usize;
        let mut high = self.row_heights.len();
        while low < high {
            let mid = (low + high) / 2;
            if self
                .row_offsets
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

    fn end_index_for_offset(&self, offset: Pixels) -> usize {
        let mut low = 0usize;
        let mut high = self.row_heights.len();
        while low < high {
            let mid = (low + high) / 2;
            if self.row_offsets.get(mid).copied().unwrap_or(Pixels::ZERO) < offset {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low
    }

    fn visible_range(&self, scroll_top: Pixels, viewport_height: Pixels) -> (usize, usize, usize) {
        if self.row_heights.is_empty() || viewport_height <= Pixels::ZERO {
            let end = self.row_heights.len().min(INITIAL_RENDER_COUNT);
            return (0, 0, end);
        }

        let padding_top = px(LIST_PADDING_TOP);
        let content_top = if scroll_top > padding_top {
            scroll_top - padding_top
        } else {
            Pixels::ZERO
        };
        let content_bottom = content_top + viewport_height;
        let visible_start = self.index_for_offset(content_top);
        let overscan = px(OVERSCAN_PX);
        let render_start = self.index_for_offset(if content_top > overscan {
            content_top - overscan
        } else {
            Pixels::ZERO
        });
        let render_end = self.end_index_for_offset(content_bottom + overscan);

        (visible_start, render_start, render_end)
    }

    fn update_row_heights(
        &mut self,
        render_start: usize,
        visible_start: usize,
        bounds: &[Bounds<Pixels>],
    ) -> bool {
        if self.scroll_drag.is_some() {
            return false;
        }

        let mut changed = false;
        let mut scroll_delta = Pixels::ZERO;
        for (offset, bound) in bounds.iter().enumerate() {
            let index = render_start + offset;
            if index >= self.row_heights.len() {
                break;
            }
            let new_height = bound.size.height;
            if new_height <= Pixels::ZERO {
                continue;
            }
            let old_height = self.row_heights[index];
            let delta = new_height - old_height;
            if delta.abs() > px(HEIGHT_EPS) {
                if index < visible_start {
                    scroll_delta += delta;
                }
                self.set_row_height(index, new_height);
                self.row_measured[index] = true;
                changed = true;
            } else if !self.row_measured[index] {
                self.row_measured[index] = true;
            }
        }

        if scroll_delta != Pixels::ZERO {
            let offset = self.scroll_handle.offset();
            self.scroll_handle
                .set_offset(point(offset.x, offset.y - scroll_delta));
        }

        changed
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
        self.mark_scrollbar_interaction();
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
            self.mark_scrollbar_interaction();
            self.scroll_settle_pending = true;
            cx.notify();
        }
    }

    fn mark_scrollbar_interaction(&mut self) {
        self.scrollbar_last_interaction = Some(Instant::now());
    }

    fn scrollbar_opacity(&self, now: Instant) -> f32 {
        if self.scroll_drag.is_some() || self.scrollbar_hovered {
            return 1.0;
        }
        let Some(last) = self.scrollbar_last_interaction else {
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
            .text_size(px(TIME_TEXT_SIZE))
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

        let mut line_stack = div().flex().flex_col().gap(px(2.0)).min_w(px(0.0));
        if entry.lines.is_empty() {
            line_stack = line_stack.child(
                div()
                    .text_size(px(BODY_TEXT_SIZE))
                    .text_color(text_color)
                    .child(""),
            );
        } else {
            for line in &entry.lines {
                line_stack = line_stack.child(
                    div()
                        .text_size(px(BODY_TEXT_SIZE))
                        .text_color(text_color)
                        .child(line.clone()),
                );
            }
        }

        div()
            .id(("detection-subtitles-row", entry.id))
            .flex()
            .flex_col()
            .gap(px(ROW_INTERNAL_GAP))
            .w_full()
            .min_w(px(0.0))
            .pt(px(ROW_TOP_PADDING))
            .pb(px(ROW_BOTTOM_PADDING + ROW_ITEM_SPACING))
            .px(px(ROW_HORIZONTAL_PADDING))
            .border_b(px(1.0))
            .border_color(divider_color)
            .child(time_row)
            .child(line_stack)
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
            .id(("detection-subtitles-scrollbar-thumb", cx.entity_id()))
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
        self.refresh_estimated_row_height(window);
        self.schedule_scroll_refresh(window, cx);

        let list_body = if self.subtitles.is_empty() {
            div()
                .flex_1()
                .min_h(px(0.0))
                .child(self.empty_placeholder(cx))
                .into_any_element()
        } else {
            let viewport_height = self.scroll_handle.bounds().size.height;
            let scroll_top = (-self.scroll_handle.offset().y).max(Pixels::ZERO);
            let (visible_start, render_start, render_end) =
                self.visible_range(scroll_top, viewport_height);
            let total_height = self.row_offsets.last().copied().unwrap_or(Pixels::ZERO);
            let render_top = self
                .row_offsets
                .get(render_start)
                .copied()
                .unwrap_or(Pixels::ZERO);
            let render_bottom = self
                .row_offsets
                .get(render_end)
                .copied()
                .unwrap_or(total_height);

            let top_spacer_height = px(LIST_PADDING_TOP) + render_top;
            let bottom_spacer_height = px(LIST_PADDING_BOTTOM) + (total_height - render_bottom);

            let mut rows = div().flex().flex_col().w_full().min_w(px(0.0));
            for index in render_start..render_end {
                if let Some(entry) = self.subtitles.get(index) {
                    rows = rows.child(self.subtitle_row(entry, cx));
                }
            }

            let handle = cx.entity();
            rows = rows.on_children_prepainted(move |bounds, _window, cx| {
                handle.update(cx, |this, cx| {
                    if this.update_row_heights(render_start, visible_start, &bounds) {
                        this.scroll_refresh_pending = true;
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
                this.mark_scrollbar_interaction();
                cx.notify();
            }))
            .on_hover(cx.listener(|this, hovered, _window, cx| {
                this.scrollbar_hovered = *hovered;
                this.mark_scrollbar_interaction();
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

        let now = Instant::now();
        let opacity = self.scrollbar_opacity(now);

        if let Some(target) = metrics {
            let metrics = if self.scroll_drag.is_some() {
                target
            } else {
                self.animated_scrollbar_metrics(target, window)
            };
            if let Some(scrollbar) =
                self.scrollbar_overlay(metrics, opacity, self.scroll_drag.is_some(), cx)
            {
                container = container.child(scrollbar);
            }
            self.last_scrollbar_metrics = Some(metrics);
        } else {
            self.scrollbar_animation = None;
            self.scroll_settle_pending = false;
            self.last_scrollbar_metrics = None;
            self.scrollbar_last_interaction = None;
        }

        if self.scrollbar_last_interaction.is_some()
            && !self.scrollbar_hovered
            && self.scroll_drag.is_none()
        {
            if opacity > f32::EPSILON {
                window.request_animation_frame();
            } else {
                self.scrollbar_last_interaction = None;
            }
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
const SCROLLBAR_FADE_DELAY_MS: u64 = 1000;
const SCROLLBAR_FADE_MS: u64 = 200;
const DEFAULT_ESTIMATED_ROW_HEIGHT: f32 = 60.0;
const ESTIMATED_BODY_LINES: f32 = 2.0;
const LIST_PADDING_TOP: f32 = 4.0;
const LIST_PADDING_BOTTOM: f32 = 4.0;
const ROW_TOP_PADDING: f32 = 2.0;
const ROW_BOTTOM_PADDING: f32 = 6.0;
const ROW_ITEM_SPACING: f32 = 4.0;
const ROW_HORIZONTAL_PADDING: f32 = 2.0;
const ROW_INTERNAL_GAP: f32 = 2.0;
const TIME_TEXT_SIZE: f32 = 9.0;
const BODY_TEXT_SIZE: f32 = 11.0;
const HEIGHT_EPS: f32 = 0.5;
const OVERSCAN_PX: f32 = 160.0;
const INITIAL_RENDER_COUNT: usize = 50;

fn seek_target(start_ms: f64) -> Option<Duration> {
    if !start_ms.is_finite() {
        return None;
    }
    let target_ms = (start_ms + 100.0).max(0.0);
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
