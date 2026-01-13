use gpui::{
    AnyWindowHandle, App, Global, Menu, MenuItem, SharedString, SystemMenuType, WindowHandle,
    actions,
};

use crate::gui::app::MainWindow;

actions!(
    subtitle_fast_menu,
    [Quit, OpenSettings, AddTask, RemoveTask, Help]
);

pub(crate) struct MainWindowState {
    handle: AnyWindowHandle,
}

impl Global for MainWindowState {}

pub fn set_main_window(handle: AnyWindowHandle, cx: &mut App) {
    cx.set_global(MainWindowState { handle });
}

fn active_main_window(cx: &mut App) -> Option<WindowHandle<MainWindow>> {
    if let Some(global) = cx.try_global::<MainWindowState>() {
        if let Some(handle) = global.handle.downcast::<MainWindow>() {
            return Some(handle);
        }
    }

    cx.active_window()
        .and_then(|window| window.downcast::<MainWindow>())
}

/// Registers handlers for app-wide menu actions.
pub fn register_actions(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());

    cx.on_action(|_: &OpenSettings, cx| {
        let Some(window) = active_main_window(cx) else {
            return;
        };
        let _ = window.update(cx, |this, window, cx| {
            this.open_config_window(window, cx);
        });
    });

    cx.on_action(|_: &AddTask, cx| {
        let Some(window) = active_main_window(cx) else {
            return;
        };
        let _ = window.update(cx, |this, window, cx| {
            this.prompt_for_video(window, true, cx);
        });
    });

    cx.on_action(|_: &RemoveTask, cx| {
        let Some(window) = active_main_window(cx) else {
            return;
        };
        let _ = window.update(cx, |this, _window, cx| {
            this.request_remove_active_session(cx);
        });
    });

    cx.on_action(|_: &Help, cx| {
        let Some(window) = active_main_window(cx) else {
            return;
        };
        let _ = window.update(cx, |this, _window, cx| {
            this.open_help_window(cx);
        });
    });
}

/// Sets the menu bar for non-macOS platforms.
pub fn set_app_menus(cx: &mut App) {
    cx.set_menus(build_menus(SharedString::from("Menu"), false));
}

/// Sets the macOS menu bar using native menus.
pub fn set_macos_menus(cx: &mut App) {
    cx.set_menus(build_menus(SharedString::from("subtitle-fast"), true));
}

fn build_menus(app_menu_title: SharedString, include_services: bool) -> Vec<Menu> {
    let mut app_items = vec![MenuItem::action("Settings...", OpenSettings)];
    if include_services {
        app_items.push(MenuItem::separator());
        app_items.push(MenuItem::os_submenu("Services", SystemMenuType::Services));
    }
    app_items.push(MenuItem::separator());
    app_items.push(MenuItem::action("Quit subtitle-fast", Quit));

    vec![
        Menu {
            name: app_menu_title.into(),
            items: app_items,
        },
        Menu {
            name: "Task".into(),
            items: vec![
                MenuItem::action("Add Task", AddTask),
                MenuItem::action("Remove Task", RemoveTask),
            ],
        },
        Menu {
            name: "Help".into(),
            items: vec![MenuItem::action("Help", Help)],
        },
    ]
}
