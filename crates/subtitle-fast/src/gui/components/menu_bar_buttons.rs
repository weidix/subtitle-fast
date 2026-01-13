use std::sync::Arc;

use gpui::prelude::*;
use gpui::{App, Bounds, MouseButton, OwnedMenu, Pixels, RenderOnce, Window, div, hsla, px};

use crate::gui::icons::{Icon, icon_sm};

const MENU_BUTTON_HEIGHT: f32 = 28.0;
const MENU_BUTTON_RADIUS: f32 = 6.0;

#[derive(Clone)]
pub struct MenuBarButtonsCallbacks {
    pub on_button_click: Arc<dyn Fn(usize, &mut Window, &mut App) + Send + Sync>,
    pub on_button_bounds: Arc<dyn Fn(usize, Option<Bounds<Pixels>>, &mut App) + Send + Sync>,
    pub on_bar_bounds: Arc<dyn Fn(Option<Bounds<Pixels>>, &mut App) + Send + Sync>,
}

#[derive(IntoElement)]
pub struct MenuBarButtons {
    menus: Vec<OwnedMenu>,
    open_menu: Option<usize>,
    callbacks: MenuBarButtonsCallbacks,
}

impl MenuBarButtons {
    pub fn new(
        menus: Vec<OwnedMenu>,
        open_menu: Option<usize>,
        callbacks: MenuBarButtonsCallbacks,
    ) -> Self {
        Self {
            menus,
            open_menu,
            callbacks,
        }
    }

    fn display_menu_name(name: &str) -> &str {
        if name == "subtitle-fast" {
            "Menu"
        } else {
            name
        }
    }
}

impl RenderOnce for MenuBarButtons {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let text_color = hsla(0.0, 0.0, 1.0, 0.82);
        let hover_bg = hsla(0.0, 0.0, 1.0, 0.08);
        let active_bg = hsla(0.0, 0.0, 1.0, 0.14);

        let callbacks = self.callbacks.clone();
        let menu_buttons = self
            .menus
            .iter()
            .enumerate()
            .map(|(index, menu)| {
                let is_open = self.open_menu == Some(index);
                let label = Self::display_menu_name(menu.name.as_ref()).to_string();
                let on_button_click = callbacks.on_button_click.clone();
                let button = div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(MENU_BUTTON_HEIGHT))
                    .px(px(10.0))
                    .rounded(px(MENU_BUTTON_RADIUS))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover_bg))
                    .bg(if is_open {
                        active_bg
                    } else {
                        hsla(0.0, 0.0, 0.0, 0.0)
                    })
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_color)
                            .child(label),
                    )
                    .child(icon_sm(Icon::ChevronDown, text_color))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        (on_button_click)(index, window, cx);
                    });

                let on_button_bounds = callbacks.on_button_bounds.clone();
                div()
                    .on_children_prepainted(move |bounds, _window, cx| {
                        let bounds = bounds.first().copied();
                        (on_button_bounds)(index, bounds, cx);
                    })
                    .child(button)
            })
            .collect::<Vec<_>>();

        let on_bar_bounds = callbacks.on_bar_bounds.clone();
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .on_children_prepainted(move |bounds, _window, cx| {
                let bounds = bounds.first().copied();
                (on_bar_bounds)(bounds, cx);
            })
            .children(menu_buttons)
    }
}
