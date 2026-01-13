use gpui::{
    AnyWindowHandle, App, Context, Global, Menu, MenuItem, SharedString, SystemMenuType, Window,
    WindowHandle, actions,
};

use crate::gui::app::MainWindow;
use crate::gui::components::DetectionRunState;
use crate::gui::session::{SessionId, VideoSession};

actions!(
    subtitle_fast_menu,
    [Quit, OpenSettings, AddTask, RemoveTask, ToggleTask, Help]
);

#[derive(Clone, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct RemoveSpecificTask {
    pub session_id: SessionId,
}

#[derive(Clone, PartialEq, gpui::Action)]
#[action(no_json)]
pub struct ToggleSpecificTask {
    pub session_id: SessionId,
}

pub(crate) struct MainWindowState {
    handle: AnyWindowHandle,
}

impl Global for MainWindowState {}

pub fn set_main_window(handle: AnyWindowHandle, cx: &mut App) {
    cx.set_global(MainWindowState { handle });
}

fn defer_main_window_action(
    cx: &mut App,
    update: impl FnOnce(&mut MainWindow, &mut Window, &mut Context<MainWindow>) + 'static,
) {
    let Some(window) = active_main_window(cx) else {
        return;
    };

    cx.defer(move |cx| {
        if let Err(err) = window.update(cx, update) {
            eprintln!("menu action failed to update main window: {err}");
        }
    });
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
        defer_main_window_action(cx, |this, window, cx| {
            this.open_config_window(window, cx);
        });
    });

    cx.on_action(|_: &AddTask, cx| {
        defer_main_window_action(cx, |this, window, cx| {
            this.prompt_for_video(window, true, cx);
        });
    });

    cx.on_action(|_: &RemoveTask, cx| {
        defer_main_window_action(cx, |this, _window, cx| {
            this.request_remove_active_session(cx);
        });
    });

    cx.on_action(|_: &ToggleTask, cx| {
        defer_main_window_action(cx, |this, _window, cx| {
            this.toggle_active_session_state(cx);
        });
    });

    cx.on_action(|action: &RemoveSpecificTask, cx| {
        let session_id = action.session_id;
        defer_main_window_action(cx, move |this, _window, cx| {
            this.request_remove_session(session_id, cx);
        });
    });

    cx.on_action(|action: &ToggleSpecificTask, cx| {
        let session_id = action.session_id;
        defer_main_window_action(cx, move |this, _window, cx| {
            this.toggle_session_state(session_id, cx);
        });
    });

    cx.on_action(|_: &Help, cx| {
        defer_main_window_action(cx, |this, _window, cx| {
            this.open_help_window(cx);
        });
    });
}

/// Sets the menu bar for non-macOS platforms.
pub fn set_app_menus(cx: &mut App, sessions: &[VideoSession]) {
    cx.set_menus(build_menus(SharedString::from("Menu"), false, sessions));
}

/// Sets the macOS menu bar using native menus.
pub fn set_macos_menus(cx: &mut App, sessions: &[VideoSession]) {
    cx.set_menus(build_menus(
        SharedString::from("subtitle-fast"),
        true,
        sessions,
    ));
}

fn build_menus(
    app_menu_title: SharedString,
    include_services: bool,
    sessions: &[VideoSession],
) -> Vec<Menu> {
    let mut app_items = vec![MenuItem::action("Settings...", OpenSettings).with_icon("gear")];
    if include_services {
        app_items.push(MenuItem::separator());
        app_items.push(MenuItem::os_submenu("Services", SystemMenuType::Services));
    }
    app_items.push(MenuItem::separator());
    app_items.push(MenuItem::action("Quit subtitle-fast", Quit));

    let remove_task_menu = if sessions.is_empty() {
        MenuItem::action("Remove Task", RemoveTask).with_icon("trash")
    } else {
        MenuItem::submenu(Menu {
            name: "Remove Task".into(),
            icon: Some("trash".into()),
            items: sessions
                .iter()
                .map(|session| {
                    let run_state = session.detection.run_state();
                    let progress = session.detection.progress_snapshot();
                    let icon_name = if progress.completed {
                        "checkmark.circle"
                    } else {
                        match run_state {
                            DetectionRunState::Running => "play.fill",
                            DetectionRunState::Paused => "pause.fill",
                            DetectionRunState::Idle => "film",
                        }
                    };

                    MenuItem::action(
                        session.label.clone(),
                        RemoveSpecificTask {
                            session_id: session.id,
                        },
                    )
                    .with_icon(icon_name)
                })
                .collect(),
        })
    };

    let toggle_task_menu = if sessions.is_empty() {
        MenuItem::action("Toggle Task State", ToggleTask).with_icon("bolt")
    } else {
        MenuItem::submenu(Menu {
            name: "Toggle Task State".into(),
            icon: Some("bolt".into()),
            items: sessions
                .iter()
                .map(|session| {
                    let run_state = session.detection.run_state();
                    let progress = session.detection.progress_snapshot();
                    let icon_name = if progress.completed {
                        "checkmark.circle"
                    } else {
                        match run_state {
                            DetectionRunState::Running => "pause.fill",
                            DetectionRunState::Paused => "play.fill",
                            DetectionRunState::Idle => "play.fill",
                        }
                    };

                    MenuItem::action(
                        session.label.clone(),
                        ToggleSpecificTask {
                            session_id: session.id,
                        },
                    )
                    .with_icon(icon_name)
                })
                .collect(),
        })
    };

    vec![
        Menu {
            name: app_menu_title.into(),
            icon: None,
            items: app_items,
        },
        Menu {
            name: "Task".into(),
            icon: None,
            items: vec![
                MenuItem::action("Add Task", AddTask).with_icon("plus"),
                toggle_task_menu,
                remove_task_menu,
            ],
        },
        Menu {
            name: "Help".into(),
            icon: None,
            items: vec![MenuItem::action("Help", Help).with_icon("questionmark.circle")],
        },
    ]
}
