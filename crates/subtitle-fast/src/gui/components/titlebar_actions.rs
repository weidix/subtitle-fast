use std::sync::Arc;

use gpui::prelude::*;
use gpui::{App, Context, MouseButton, Render, Window, div, hsla, px};

use crate::gui::icons::{Icon, icon_sm};

const BUTTON_WIDTH: f32 = 28.0;

#[derive(Clone)]
pub struct TitlebarActionsCallbacks {
    pub on_settings: Arc<dyn Fn(&mut Window, &mut App) + Send + Sync>,
    pub on_help: Arc<dyn Fn(&mut Window, &mut App) + Send + Sync>,
}

pub struct TitlebarActions {
    callbacks: TitlebarActionsCallbacks,
}

impl TitlebarActions {
    pub fn new(callbacks: TitlebarActionsCallbacks) -> Self {
        Self { callbacks }
    }

    pub fn set_callbacks(&mut self, callbacks: TitlebarActionsCallbacks, cx: &mut Context<Self>) {
        self.callbacks = callbacks;
        cx.notify();
    }
}

impl Render for TitlebarActions {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let icon_color = hsla(0.0, 0.0, 0.9, 1.0);
        let hover_bg = hsla(0.0, 0.0, 1.0, 0.12);

        let settings = {
            let on_settings = self.callbacks.on_settings.clone();
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(BUTTON_WIDTH))
                .h_full()
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg))
                .child(icon_sm(Icon::SlidersHorizontal, icon_color))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    (on_settings)(window, cx);
                })
        };

        let help = {
            let on_help = self.callbacks.on_help.clone();
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(BUTTON_WIDTH))
                .h_full()
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg))
                .child(icon_sm(Icon::Info, icon_color))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    (on_help)(window, cx);
                })
        };

        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(settings)
            .child(help)
    }
}
