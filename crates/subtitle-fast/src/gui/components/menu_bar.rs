use std::sync::Arc;

use gpui::prelude::*;
use gpui::{
    Action, Bounds, Context, DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent, OwnedMenu,
    OwnedMenuItem, Pixels, Render, Window, div, hsla, px, rgb,
};

use super::menu_bar_buttons::{MenuBarButtons, MenuBarButtonsCallbacks};
const MENU_POPUP_WIDTH: f32 = 200.0;
const MENU_POPUP_RADIUS: f32 = 8.0;
const MENU_POPUP_OFFSET_Y: f32 = 6.0;
const MENU_ITEM_HEIGHT: f32 = 28.0;

pub struct MenuBar {
    menus: Vec<OwnedMenu>,
    open_menu: Option<usize>,
    button_bounds: Vec<Option<Bounds<Pixels>>>,
    popup_bounds: Option<Bounds<Pixels>>,
    bar_bounds: Option<Bounds<Pixels>>,
}

impl MenuBar {
    pub fn new(menus: Vec<OwnedMenu>) -> Self {
        let button_bounds = vec![None; menus.len()];
        Self {
            menus,
            open_menu: None,
            button_bounds,
            popup_bounds: None,
            bar_bounds: None,
        }
    }

    pub fn set_menus(&mut self, menus: Vec<OwnedMenu>) {
        self.menus = menus;
        self.open_menu = None;
        self.popup_bounds = None;
        self.bar_bounds = None;
        self.button_bounds = vec![None; self.menus.len()];
    }

    fn toggle_menu(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.open_menu == Some(index) {
            self.open_menu = None;
        } else {
            self.open_menu = Some(index);
        }
        self.popup_bounds = None;
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.open_menu = None;
        self.popup_bounds = None;
        cx.notify();
    }

    fn menu_entries(menu: &OwnedMenu) -> Vec<MenuEntry> {
        let mut entries = Vec::new();
        Self::push_entries(&menu.items, 0, &mut entries);
        entries
    }

    fn push_entries(items: &[OwnedMenuItem], indent: usize, out: &mut Vec<MenuEntry>) {
        for item in items {
            match item {
                OwnedMenuItem::Separator => out.push(MenuEntry::Separator),
                OwnedMenuItem::Action { name, action, .. } => out.push(MenuEntry::Action {
                    name: name.clone(),
                    action: action.boxed_clone(),
                    indent,
                }),
                OwnedMenuItem::Submenu(submenu) => {
                    out.push(MenuEntry::Label {
                        name: submenu.name.to_string(),
                    });
                    Self::push_entries(&submenu.items, indent + 1, out);
                }
                OwnedMenuItem::SystemMenu(_) => {}
            }
        }
    }
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text_color = hsla(0.0, 0.0, 1.0, 0.82);
        let muted_color = hsla(0.0, 0.0, 1.0, 0.5);
        let hover_bg = hsla(0.0, 0.0, 1.0, 0.08);
        let popup_bg = rgb(0x202020);
        let popup_border = rgb(0x2f2f2f);

        if let Some(open_index) = self.open_menu {
            let handle = cx.entity();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                if event.button != MouseButton::Left {
                    return;
                }
                let position = event.position;
                let _ = handle.update(cx, |this, cx| {
                    if this.open_menu != Some(open_index) {
                        return;
                    }
                    if let Some(bounds) = this.popup_bounds.clone() {
                        if bounds.contains(&position) {
                            return;
                        }
                    }
                    if let Some(button_bounds) =
                        this.button_bounds.get(open_index).and_then(|b| b.clone())
                    {
                        if button_bounds.contains(&position) {
                            return;
                        }
                    }
                    this.close_menu(cx);
                });
            });
        }

        if let Some(open_index) = self.open_menu {
            let handle = cx.entity();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                let position = event.position;
                let _ = handle.update(cx, |this, cx| {
                    if this.open_menu != Some(open_index) {
                        return;
                    }
                    let in_popup = this
                        .popup_bounds
                        .map(|bounds| bounds.contains(&position))
                        .unwrap_or(false);
                    if in_popup {
                        return;
                    }

                    let mut hover_index = None;
                    for (index, bounds) in this.button_bounds.iter().enumerate() {
                        if bounds
                            .as_ref()
                            .is_some_and(|bounds| bounds.contains(&position))
                        {
                            hover_index = Some(index);
                            break;
                        }
                    }

                    if let Some(index) = hover_index {
                        if Some(index) != this.open_menu {
                            this.open_menu = Some(index);
                            this.popup_bounds = None;
                            cx.notify();
                        }
                        return;
                    }

                    if let (Some(button_bounds), Some(popup_bounds)) = (
                        this.button_bounds
                            .get(open_index)
                            .and_then(|bounds| *bounds),
                        this.popup_bounds,
                    ) {
                        let between_y = position.y >= button_bounds.bottom()
                            && position.y <= popup_bounds.top();
                        let between_x = position.x >= button_bounds.left()
                            && position.x <= button_bounds.right();
                        if between_x && between_y {
                            return;
                        }
                    }

                    this.close_menu(cx);
                });
            });
        }

        let handle = cx.entity();
        let menu_buttons = MenuBarButtons::new(
            self.menus.clone(),
            self.open_menu,
            MenuBarButtonsCallbacks {
                on_button_click: Arc::new(move |index, _window, cx| {
                    let _ = handle.update(cx, |this, cx| {
                        this.toggle_menu(index, cx);
                    });
                }),
                on_button_bounds: {
                    let handle = cx.entity();
                    Arc::new(move |index, bounds, cx| {
                        let _ = handle.update(cx, |this, _| {
                            if let Some(slot) = this.button_bounds.get_mut(index) {
                                *slot = bounds;
                            }
                        });
                    })
                },
                on_bar_bounds: {
                    let handle = cx.entity();
                    Arc::new(move |bounds, cx| {
                        let _ = handle.update(cx, |this, _| {
                            this.bar_bounds = bounds;
                        });
                    })
                },
            },
        );

        let mut root = div().relative().child(menu_buttons);

        if let Some(open_index) = self.open_menu {
            if let Some(menu) = self.menus.get(open_index) {
                let entries = Self::menu_entries(menu);
                let popup_left = self
                    .button_bounds
                    .get(open_index)
                    .and_then(|bounds| bounds.clone())
                    .and_then(|button_bounds| {
                        self.bar_bounds
                            .map(|bar_bounds| button_bounds.left() - bar_bounds.left())
                    })
                    .unwrap_or(px(0.0));
                let popup_top = self
                    .button_bounds
                    .get(open_index)
                    .and_then(|bounds| bounds.clone())
                    .and_then(|button_bounds| {
                        self.bar_bounds
                            .map(|bar_bounds| button_bounds.bottom() - bar_bounds.top())
                    })
                    .unwrap_or(px(0.0))
                    + px(MENU_POPUP_OFFSET_Y);

                let handle = cx.entity();
                let popup_bounds_handle = handle.clone();
                let popup_content = entries.into_iter().map(|entry| match entry {
                    MenuEntry::Separator => div()
                        .h(px(1.0))
                        .bg(popup_border)
                        .mx(px(8.0))
                        .my(px(4.0))
                        .into_any_element(),
                    MenuEntry::Label { name } => div()
                        .px(px(10.0))
                        .py(px(6.0))
                        .text_size(px(11.0))
                        .text_color(muted_color)
                        .child(name)
                        .into_any_element(),
                    MenuEntry::Action {
                        name,
                        action,
                        indent,
                    } => {
                        let enabled = cx.is_action_available(action.as_ref());
                        let indent_padding = px(10.0 + indent as f32 * 12.0);
                        let mut row = div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .h(px(MENU_ITEM_HEIGHT))
                            .px(indent_padding)
                            .text_size(px(12.0))
                            .text_color(if enabled { text_color } else { muted_color })
                            .child(name);

                        if enabled {
                            let action = action.boxed_clone();
                            let handle = handle.clone();
                            row = row
                                .cursor_pointer()
                                .hover(move |style| style.bg(hover_bg))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |_, _event, _window, cx| {
                                        cx.dispatch_action(action.as_ref());
                                        let _ = handle.update(cx, |this, cx| {
                                            this.close_menu(cx);
                                        });
                                    }),
                                );
                        }

                        row.into_any_element()
                    }
                });

                let popup = div()
                    .absolute()
                    .top(popup_top)
                    .left(popup_left)
                    .w(px(MENU_POPUP_WIDTH))
                    .bg(popup_bg)
                    .border_1()
                    .border_color(popup_border)
                    .rounded(px(MENU_POPUP_RADIUS))
                    .shadow(vec![gpui::BoxShadow {
                        color: hsla(0.0, 0.0, 0.0, 0.35),
                        offset: gpui::point(px(0.0), px(4.0)),
                        blur_radius: px(8.0),
                        spread_radius: px(0.0),
                    }])
                    .occlude()
                    .children(popup_content);

                let popup_wrapper = div().on_children_prepainted(move |bounds, _window, cx| {
                    let bounds = bounds.first().copied();
                    let _ = popup_bounds_handle.update(cx, |this, _| {
                        this.popup_bounds = bounds;
                    });
                });

                root = root.child(popup_wrapper.child(popup));
            }
        }

        root
    }
}

enum MenuEntry {
    Separator,
    Label {
        name: String,
    },
    Action {
        name: String,
        action: Box<dyn Action>,
        indent: usize,
    },
}
