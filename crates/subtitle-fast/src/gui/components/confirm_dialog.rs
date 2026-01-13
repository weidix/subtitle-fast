use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement, MouseButton, Render, SharedString,
    Window, div, hsla, px, rgb,
};

pub enum ConfirmDialogTitle {
    Text(SharedString),
    Element(Arc<dyn Fn() -> AnyElement>),
}

impl ConfirmDialogTitle {
    pub fn text(text: impl Into<SharedString>) -> Self {
        Self::Text(text.into())
    }

    pub fn element(builder: impl Fn() -> AnyElement + 'static) -> Self {
        Self::Element(Arc::new(builder))
    }
}

#[derive(Clone, Copy)]
pub enum ConfirmDialogButtonStyle {
    Primary,
    Secondary,
    Danger,
}

pub struct ConfirmDialogButton {
    pub label: SharedString,
    pub style: ConfirmDialogButtonStyle,
    pub close_on_click: bool,
    pub on_click: Arc<dyn Fn(&mut Window, &mut Context<ConfirmDialog>)>,
}

impl ConfirmDialogButton {
    pub fn new(
        label: impl Into<SharedString>,
        style: ConfirmDialogButtonStyle,
        close_on_click: bool,
        on_click: Arc<dyn Fn(&mut Window, &mut Context<ConfirmDialog>)>,
    ) -> Self {
        Self {
            label: label.into(),
            style,
            close_on_click,
            on_click,
        }
    }
}

pub struct ConfirmDialogConfig {
    pub title: ConfirmDialogTitle,
    pub message: SharedString,
    pub buttons: Vec<ConfirmDialogButton>,
    pub show_backdrop: bool,
    pub backdrop_color: Hsla,
    pub close_on_outside: bool,
}

pub struct ConfirmDialog {
    config: Option<ConfirmDialogConfig>,
}

impl ConfirmDialog {
    pub fn new() -> Self {
        Self { config: None }
    }

    pub fn open(&mut self, config: ConfirmDialogConfig, cx: &mut Context<Self>) {
        self.config = Some(config);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.config.is_some() {
            self.config = None;
            cx.notify();
        }
    }
}

#[derive(Clone, Copy)]
struct ConfirmDialogButtonTheme {
    bg: Hsla,
    hover_bg: Hsla,
    border: Hsla,
    text: Hsla,
}

impl ConfirmDialogButtonStyle {
    fn theme(self) -> ConfirmDialogButtonTheme {
        match self {
            Self::Primary => ConfirmDialogButtonTheme {
                bg: hsla(0.0, 0.0, 0.92, 1.0),
                hover_bg: hsla(0.0, 0.0, 1.0, 1.0),
                border: hsla(0.0, 0.0, 0.8, 1.0),
                text: hsla(0.0, 0.0, 0.12, 1.0),
            },
            Self::Secondary => ConfirmDialogButtonTheme {
                bg: hsla(0.0, 0.0, 0.18, 1.0),
                hover_bg: hsla(0.0, 0.0, 0.24, 1.0),
                border: hsla(0.0, 0.0, 0.3, 1.0),
                text: hsla(0.0, 0.0, 0.85, 1.0),
            },
            Self::Danger => ConfirmDialogButtonTheme {
                bg: hsla(0.0, 0.72, 0.51, 1.0),
                hover_bg: hsla(0.0, 0.78, 0.56, 1.0),
                border: hsla(0.0, 0.6, 0.45, 1.0),
                text: hsla(0.0, 0.0, 1.0, 1.0),
            },
        }
    }
}

impl Render for ConfirmDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(config) = self.config.as_ref() else {
            return div();
        };

        let backdrop_color = if config.show_backdrop {
            config.backdrop_color
        } else {
            hsla(0.0, 0.0, 0.0, 0.0)
        };
        let close_on_outside = config.close_on_outside;

        let title = match &config.title {
            ConfirmDialogTitle::Text(text) => div()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(hsla(0.0, 0.0, 0.92, 1.0))
                .child(text.clone())
                .into_any_element(),
            ConfirmDialogTitle::Element(builder) => div()
                .text_size(px(14.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(hsla(0.0, 0.0, 0.92, 1.0))
                .child(builder())
                .into_any_element(),
        };

        let message = div()
            .text_size(px(12.0))
            .text_color(hsla(0.0, 0.0, 0.72, 1.0))
            .child(config.message.clone());

        let mut buttons_row = div().flex().items_center().justify_end().gap(px(8.0));

        for button in &config.buttons {
            let theme = button.style.theme();
            let label = button.label.clone();
            let close_on_click = button.close_on_click;
            let on_click = button.on_click.clone();

            let button_view = div()
                .flex()
                .items_center()
                .justify_center()
                .h(px(28.0))
                .px(px(14.0))
                .rounded(px(6.0))
                .bg(theme.bg)
                .border_1()
                .border_color(theme.border)
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text)
                .cursor_pointer()
                .hover(move |style| style.bg(theme.hover_bg))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, window, cx| {
                        (on_click)(window, cx);
                        if close_on_click {
                            this.close(cx);
                        }
                    }),
                )
                .child(label);

            buttons_row = buttons_row.child(button_view);
        }

        let dialog = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .w(px(380.0))
            .p(px(16.0))
            .rounded(px(12.0))
            .bg(rgb(0x1f1f1f))
            .border_1()
            .border_color(rgb(0x2f2f2f))
            .child(title)
            .child(message)
            .child(buttons_row)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _event, _window, cx| {
                    cx.stop_propagation();
                }),
            );

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(backdrop_color)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    cx.stop_propagation();
                    if close_on_outside {
                        this.close(cx);
                    }
                }),
            )
            .child(dialog)
    }
}
