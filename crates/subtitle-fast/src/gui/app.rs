use futures_channel::mpsc::unbounded;
use futures_util::StreamExt;
use gpui::prelude::*;
use gpui::*;
use rust_embed::RustEmbed;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use subtitle_fast_validator::subtitle_detection::SubtitleDetectorKind;
use tokio::sync::oneshot;

use crate::gui::components::{
    CollapseDirection, ColorPicker, ConfigWindow, ConfirmDialog, ConfirmDialogButton,
    ConfirmDialogButtonStyle, ConfirmDialogConfig, ConfirmDialogTitle, DetectedSubtitlesList,
    DetectionControls, DetectionHandle, DetectionMetrics, DetectionRunState, DetectionSidebar,
    DetectionSidebarHost, DragRange, DraggableEdge, HelpWindow, Sidebar, SidebarConfig,
    SidebarHandle, SubtitleEditorWindow, TaskSidebar, TaskSidebarCallbacks, Titlebar,
    TitlebarActions, TitlebarActionsCallbacks, VideoControls, VideoLumaControls, VideoLumaHandle,
    VideoPlayer, VideoPlayerControlHandle, VideoPlayerInfoHandle, VideoRoiHandle, VideoRoiOverlay,
    VideoToolbar,
};
use crate::gui::icons::{Icon, icon_md, icon_sm};
use crate::gui::menus::{self, OpenSubtitleEditor};
use crate::gui::runtime;
use crate::gui::session::{SessionHandle, SessionId, VideoSession};

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*.svg"]
struct EmbeddedAssets;

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        if let Some(asset) = EmbeddedAssets::get(path) {
            return Ok(Some(asset.data));
        }

        Ok(None)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries: Vec<SharedString> = EmbeddedAssets::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect();
        entries.sort();
        entries.dedup();
        Ok(entries)
    }
}

pub struct SubtitleFastApp;

impl SubtitleFastApp {
    pub fn new(_cx: &mut App) -> Self {
        Self
    }

    pub fn open_window(&self, cx: &mut App) -> WindowHandle<MainWindow> {
        let bounds = Bounds::centered(None, size(px(1200.0), px(800.0)), cx);

        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(960.0), px(640.0))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("subtitle-fast".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    window_decorations: Some(WindowDecorations::Client),
                    ..Default::default()
                },
                move |window, cx| {
                    let sessions = SessionHandle::new();
                    let titlebar = cx.new(|_| Titlebar::new("main-titlebar", ""));
                    let titlebar_for_window = titlebar.clone();
                    let task_sidebar_view = cx.new(|_| {
                        TaskSidebar::new(
                            sessions.clone(),
                            TaskSidebarCallbacks {
                                on_add: Arc::new(|_, _| {}),
                                on_select: Arc::new(|_, _, _| {}),
                                on_cancel: Arc::new(|_, _, _| {}),
                                on_remove: Arc::new(|_, _, _| {}),
                            },
                        )
                    });
                    let (left_panel, left_panel_handle) = Sidebar::create(
                        SidebarConfig {
                            edge: DraggableEdge::Right,
                            range: DragRange::new(px(200.0), px(480.0)),
                            collapse_direction: CollapseDirection::Left,
                            collapsed_width: px(0.0),
                            collapse_duration: Duration::from_millis(160),
                            drag_hit_thickness: px(SIDEBAR_DRAG_HIT_THICKNESS),
                        },
                        {
                            let task_sidebar_view = task_sidebar_view.clone();
                            move || task_sidebar_content(task_sidebar_view.clone())
                        },
                        cx,
                    );
                    let detection_sidebar_host = cx.new(DetectionSidebarHost::new);
                    let (right_panel, _) = Sidebar::create(
                        SidebarConfig {
                            edge: DraggableEdge::Left,
                            range: DragRange::new(px(240.0), px(520.0)),
                            collapse_direction: CollapseDirection::Right,
                            collapsed_width: px(0.0),
                            collapse_duration: Duration::from_millis(160),
                            drag_hit_thickness: px(SIDEBAR_DRAG_HIT_THICKNESS),
                        },
                        {
                            let detection_sidebar_host = detection_sidebar_host.clone();
                            move || detection_sidebar_content(detection_sidebar_host.clone())
                        },
                        cx,
                    );
                    let (luma_controls, luma_handle) = VideoLumaControls::new();
                    let luma_controls_view = cx.new(|_| luma_controls);
                    let controls_view = cx.new(|_| VideoControls::new());
                    let (color_picker, color_picker_handle) = ColorPicker::new();
                    let color_picker_view = cx.new(|_| color_picker);
                    let toolbar_view = cx.new(|_| VideoToolbar::new());
                    let confirm_dialog_view = cx.new(|_| ConfirmDialog::new());
                    let titlebar_actions = Some(cx.new(|_| {
                        TitlebarActions::new(TitlebarActionsCallbacks {
                            on_settings: Arc::new(|_, _| {}),
                            on_help: Arc::new(|_, _| {}),
                        })
                    }));
                    let titlebar_actions_view = titlebar_actions.clone();
                    let (roi_overlay, roi_handle) = VideoRoiOverlay::new();
                    let roi_overlay_view = cx.new(|_| roi_overlay);
                    toolbar_view.update(cx, |toolbar_view, cx| {
                        toolbar_view.set_luma_controls(
                            Some(luma_handle.clone()),
                            Some(luma_controls_view.clone()),
                            cx,
                        );
                        toolbar_view.set_roi_overlay(Some(roi_overlay_view.clone()), cx);
                        toolbar_view.set_roi_handle(Some(roi_handle.clone()));
                        toolbar_view.set_color_picker(
                            Some(color_picker_view.clone()),
                            Some(color_picker_handle.clone()),
                            cx,
                        );
                        cx.notify();
                    });
                    roi_overlay_view.update(cx, |overlay, cx| {
                        overlay.set_color_picker(
                            Some(color_picker_view.clone()),
                            Some(color_picker_handle.clone()),
                            cx,
                        );
                    });
                    let main_window = cx.new(|_| {
                        MainWindow::new(MainWindowParts {
                            player: None,
                            titlebar: titlebar_for_window,
                            left_panel,
                            left_panel_handle,
                            right_panel,
                            sessions: sessions.clone(),
                            task_sidebar: task_sidebar_view.clone(),
                            detection_sidebar_host: detection_sidebar_host.clone(),
                            luma_handle: luma_handle.clone(),
                            toolbar_view,
                            luma_controls_view,
                            controls_view,
                            roi_overlay: roi_overlay_view,
                            roi_handle,
                            confirm_dialog: confirm_dialog_view.clone(),
                            titlebar_actions,
                        })
                    });
                    let main_close_for_should_close = main_window.downgrade();
                    window.on_window_should_close(cx, move |_window, cx| {
                        if let Some(main_window) = main_close_for_should_close.upgrade() {
                            let _ = main_window.update(cx, |this, cx| {
                                this.close_aux_windows(cx);
                            });
                        }
                        true
                    });
                    let main_close_for_titlebar = main_window.downgrade();
                    let _ = titlebar.update(cx, move |titlebar, cx| {
                        let close_action = Arc::new(move |window: &mut Window, cx: &mut App| {
                            if let Some(main_window) = main_close_for_titlebar.upgrade() {
                                let _ = main_window.update(cx, |this, cx| {
                                    this.close_aux_windows(cx);
                                });
                            }
                            window.remove_window();
                        });
                        titlebar.set_on_close(Some(close_action), cx);
                    });
                    let weak_main = main_window.downgrade();
                    let add_handle = weak_main.clone();
                    let select_handle = weak_main.clone();
                    let cancel_handle = weak_main.clone();
                    let remove_handle = weak_main.clone();
                    task_sidebar_view.update(cx, |sidebar, cx| {
                        sidebar.set_callbacks(
                            TaskSidebarCallbacks {
                                on_add: Arc::new(move |window, cx| {
                                    let Some(main_window) = add_handle.upgrade() else {
                                        return;
                                    };
                                    main_window.update(cx, |this, cx| {
                                        this.prompt_for_video(window, true, cx);
                                    });
                                }),
                                on_select: Arc::new(move |session_id, _window, cx| {
                                    let Some(main_window) = select_handle.upgrade() else {
                                        return;
                                    };
                                    main_window.update(cx, |this, cx| {
                                        this.activate_session(session_id, cx);
                                    });
                                }),
                                on_cancel: Arc::new(move |session_id, _window, cx| {
                                    let Some(main_window) = cancel_handle.upgrade() else {
                                        return;
                                    };
                                    main_window.update(cx, |this, cx| {
                                        this.request_cancel_session(session_id, cx);
                                    });
                                }),
                                on_remove: Arc::new(move |session_id, _window, cx| {
                                    let Some(main_window) = remove_handle.upgrade() else {
                                        return;
                                    };
                                    main_window.update(cx, |this, cx| {
                                        this.request_remove_session(session_id, cx);
                                    });
                                }),
                            },
                            cx,
                        );
                    });
                    let _ = titlebar_actions_view.as_ref().map(|titlebar_actions| {
                        let settings_handle = weak_main.clone();
                        let help_handle = weak_main.clone();
                        titlebar_actions.update(cx, |actions, cx| {
                            actions.set_callbacks(
                                TitlebarActionsCallbacks {
                                    on_settings: Arc::new(move |window, cx| {
                                        let Some(main_window) = settings_handle.upgrade() else {
                                            return;
                                        };
                                        main_window.update(cx, |this, cx| {
                                            this.open_config_window(window, cx);
                                        });
                                    }),
                                    on_help: Arc::new(move |_window, cx| {
                                        let Some(main_window) = help_handle.upgrade() else {
                                            return;
                                        };
                                        main_window.update(cx, |this, cx| {
                                            this.open_help_window(cx);
                                        });
                                    }),
                                },
                                cx,
                            );
                        })
                    });
                    main_window
                },
            )
            .unwrap();

        let main_window = gpui::AnyWindowHandle::from(window);
        menus::set_main_window(main_window, cx);
        main_window
            .downcast::<MainWindow>()
            .expect("main window type mismatch")
    }
}

fn resolve_overlay_detector_kind() -> Option<SubtitleDetectorKind> {
    let settings = crate::settings::resolve_gui_settings().ok()?;
    Some(settings.detection.detector)
}

const SUPPORTED_VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "m4v", "mpg", "mpeg", "ts",
];
const VIDEO_AREA_HEIGHT_RATIO: f32 = 0.6;
const SIDEBAR_DRAG_HIT_THICKNESS: f32 = 6.0;
const SIDEBAR_BORDER_WIDTH: f32 = 1.1;
const SIDEBAR_BORDER_COLOR: u32 = 0x2b2b2b;

fn task_sidebar_content(panel_view: Entity<TaskSidebar>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .size_full()
        .child(panel_view)
        .into_any_element()
}

fn detection_sidebar_content(panel_view: Entity<DetectionSidebarHost>) -> AnyElement {
    let border_width = px(SIDEBAR_BORDER_WIDTH);
    let border_color = rgb(SIDEBAR_BORDER_COLOR);
    div()
        .flex()
        .flex_col()
        .size_full()
        .bg(rgb(0x1a1a1a))
        .border_l(border_width)
        .border_color(border_color)
        .child(panel_view)
        .into_any_element()
}

struct MenuRefreshListener {
    _ui_task: Task<()>,
    stop_tx: Option<oneshot::Sender<()>>,
}

impl MenuRefreshListener {
    fn new(ui_task: Task<()>, stop_tx: oneshot::Sender<()>) -> Self {
        Self {
            _ui_task: ui_task,
            stop_tx: Some(stop_tx),
        }
    }

    fn stop(&mut self) {
        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
    }
}

impl Drop for MenuRefreshListener {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct MainWindow {
    player: Option<Entity<VideoPlayer>>,
    controls: Option<VideoPlayerControlHandle>,
    video_info: Option<VideoPlayerInfoHandle>,
    video_bounds: Option<Bounds<Pixels>>,
    replay_visible: bool,
    replay_dismissed: bool,
    titlebar: Entity<Titlebar>,
    left_panel: Entity<Sidebar>,
    _left_panel_handle: SidebarHandle,
    right_panel: Entity<Sidebar>,
    sessions: SessionHandle,
    active_session: Option<SessionId>,
    task_sidebar: Entity<TaskSidebar>,
    detection_sidebar_host: Entity<DetectionSidebarHost>,
    luma_handle: VideoLumaHandle,
    toolbar_view: Entity<VideoToolbar>,
    luma_controls_view: Entity<VideoLumaControls>,
    controls_view: Entity<VideoControls>,
    roi_overlay: Entity<VideoRoiOverlay>,
    roi_handle: VideoRoiHandle,
    confirm_dialog: Entity<ConfirmDialog>,
    titlebar_actions: Option<Entity<TitlebarActions>>,
    menu_refresh_listeners: HashMap<SessionId, MenuRefreshListener>,
    config_window: Option<WindowHandle<ConfigWindow>>,
    help_window: Option<WindowHandle<HelpWindow>>,
    subtitle_editor_windows: HashMap<SessionId, WindowHandle<SubtitleEditorWindow>>,
}

struct MainWindowParts {
    player: Option<Entity<VideoPlayer>>,
    titlebar: Entity<Titlebar>,
    left_panel: Entity<Sidebar>,
    left_panel_handle: SidebarHandle,
    right_panel: Entity<Sidebar>,
    sessions: SessionHandle,
    task_sidebar: Entity<TaskSidebar>,
    detection_sidebar_host: Entity<DetectionSidebarHost>,
    luma_handle: VideoLumaHandle,
    toolbar_view: Entity<VideoToolbar>,
    luma_controls_view: Entity<VideoLumaControls>,
    controls_view: Entity<VideoControls>,
    roi_overlay: Entity<VideoRoiOverlay>,
    roi_handle: VideoRoiHandle,
    confirm_dialog: Entity<ConfirmDialog>,
    titlebar_actions: Option<Entity<TitlebarActions>>,
}

impl MainWindow {
    fn new(parts: MainWindowParts) -> Self {
        Self {
            player: parts.player,
            controls: None,
            video_info: None,
            video_bounds: None,
            replay_visible: false,
            replay_dismissed: false,
            titlebar: parts.titlebar,
            left_panel: parts.left_panel,
            _left_panel_handle: parts.left_panel_handle,
            right_panel: parts.right_panel,
            sessions: parts.sessions,
            active_session: None,
            task_sidebar: parts.task_sidebar,
            detection_sidebar_host: parts.detection_sidebar_host,
            luma_handle: parts.luma_handle,
            toolbar_view: parts.toolbar_view,
            luma_controls_view: parts.luma_controls_view,
            controls_view: parts.controls_view,
            roi_overlay: parts.roi_overlay,
            roi_handle: parts.roi_handle,
            confirm_dialog: parts.confirm_dialog,
            titlebar_actions: parts.titlebar_actions,
            menu_refresh_listeners: HashMap::new(),
            config_window: None,
            help_window: None,
            subtitle_editor_windows: HashMap::new(),
        }
    }

    /// Opens the settings window or focuses it if already open.
    pub(crate) fn open_config_window(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.config_window
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }

        let handle = ConfigWindow::open(cx);
        let _ = handle.update(cx, |_, window, _| {
            window.activate_window();
        });
        self.config_window = Some(handle);
    }

    /// Opens the help window or focuses it if already open.
    pub(crate) fn open_help_window(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.help_window
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }

        if let Some(handle) = HelpWindow::open(cx) {
            let _ = handle.update(cx, |_, window, _| {
                window.activate_window();
            });
            self.help_window = Some(handle);
        }
    }

    fn close_aux_windows(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.config_window.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
        if let Some(handle) = self.help_window.take() {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }

        let editor_windows: Vec<WindowHandle<SubtitleEditorWindow>> = self
            .subtitle_editor_windows
            .drain()
            .map(|(_, handle)| handle)
            .collect();
        for handle in editor_windows {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        }
    }

    /// Opens the subtitle editor window or focuses it if already open.
    pub(crate) fn open_subtitle_editor_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.active_session else {
            let task = window.spawn(cx, |cx: &mut gpui::AsyncWindowContext| {
                let mut cx = cx.clone();
                async move {
                    let buttons = [PromptButton::ok("OK")];
                    let _ = cx
                        .prompt(
                            PromptLevel::Warning,
                            "No active task",
                            Some("Select or add a task before editing subtitles."),
                            &buttons,
                        )
                        .await;
                }
            });
            drop(task);
            return;
        };

        if let Some(handle) = self.subtitle_editor_windows.get(&session_id)
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        self.subtitle_editor_windows.remove(&session_id);

        let Some(session) = self.sessions.session(session_id) else {
            return;
        };

        if !session.detection.progress_snapshot().completed {
            let task = window.spawn(cx, |cx: &mut gpui::AsyncWindowContext| {
                let mut cx = cx.clone();
                async move {
                    let buttons = [PromptButton::ok("OK")];
                    let _ = cx
                        .prompt(
                            PromptLevel::Warning,
                            "Subtitle editor unavailable",
                            Some("Wait for the task to complete before editing subtitles."),
                            &buttons,
                        )
                        .await;
                }
            });
            drop(task);
            return;
        }

        if let Some(handle) = SubtitleEditorWindow::open(session, cx) {
            let _ = handle.update(cx, |_, window, _| {
                window.activate_window();
            });
            self.subtitle_editor_windows.insert(session_id, handle);
        }
    }

    fn can_open_subtitle_editor(&self) -> bool {
        let Some(session_id) = self.active_session else {
            return false;
        };
        let Some(session) = self.sessions.session(session_id) else {
            return false;
        };
        session.detection.progress_snapshot().completed
    }

    /// Prompts the user to select a video file and queues a new session.
    pub(crate) fn prompt_for_video(
        &mut self,
        window: &mut Window,
        select_new: bool,
        cx: &mut Context<Self>,
    ) {
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select video".into()),
            allowed_extensions: Some(
                SUPPORTED_VIDEO_EXTENSIONS
                    .iter()
                    .map(|ext| SharedString::new_static(ext))
                    .collect(),
            ),
        };
        let supported_detail = supported_video_extensions_detail();

        cx.spawn_in(
            window,
            move |this: WeakEntity<Self>, cx: &mut AsyncWindowContext| {
                let mut cx = cx.clone();
                async move {
                    loop {
                        let receiver =
                            match cx.update(|_, app| app.prompt_for_paths(options.clone())) {
                                Ok(receiver) => receiver,
                                Err(err) => {
                                    eprintln!("video selection failed: {err}");
                                    return;
                                }
                            };

                        let selection = match receiver.await {
                            Ok(Ok(Some(mut paths))) => paths.pop(),
                            Ok(Ok(None)) => None,
                            Ok(Err(err)) => {
                                eprintln!("video selection failed: {err}");
                                None
                            }
                            Err(err) => {
                                eprintln!("video selection canceled: {err}");
                                None
                            }
                        };

                        let Some(path) = selection else {
                            return;
                        };

                        if is_supported_video_path(&path) {
                            let _ = this.update(&mut cx, move |this, cx| {
                                this.enqueue_session(path, select_new, cx);
                            });
                            return;
                        }

                        let answers = [PromptButton::ok("OK")];
                        let _ = cx
                            .prompt(
                                PromptLevel::Warning,
                                "Unsupported video format",
                                Some(&supported_detail),
                                &answers,
                            )
                            .await;
                    }
                }
            },
        )
        .detach();
    }

    fn enqueue_session(&mut self, path: PathBuf, select_new: bool, cx: &mut Context<Self>) {
        let detection_handle = DetectionHandle::new();
        detection_handle.set_video_path(Some(path.clone()));
        detection_handle.set_luma_handle(Some(self.luma_handle.clone()));
        detection_handle.set_roi_handle(Some(self.roi_handle.clone()));
        let session_id = self.sessions.add_session(path, detection_handle);
        if let Ok(settings) = crate::settings::resolve_gui_settings() {
            self.sessions.update_settings(
                session_id,
                Some(settings.detection.target),
                Some(settings.detection.delta),
                None,
                settings.detection.roi,
            );
        }

        if select_new || self.active_session.is_none() {
            self.activate_session(session_id, cx);
        } else {
            self.notify_task_sidebar(cx);
        }
        self.refresh_app_menus(cx);
    }

    fn activate_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        if self.active_session == Some(session_id) {
            return;
        }

        self.save_active_session_state(cx);
        self.active_session = Some(session_id);
        self.sessions.set_active(session_id);
        self.notify_task_sidebar(cx);
        self.release_player(cx);

        let Some(session) = self.sessions.session(session_id) else {
            self.update_detection_sidebar(None, cx);
            return;
        };
        self.load_session(&session, cx);
        self.update_detection_sidebar(Some(session.detection.clone()), cx);
        self.refresh_app_menus(cx);
        cx.notify();
    }

    fn remove_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        if let Some(session) = self.sessions.session(session_id) {
            session.detection.cancel();
        }
        self.subtitle_editor_windows.remove(&session_id);
        if self.active_session == Some(session_id) {
            self.active_session = None;
            self.release_player(cx);
            self.update_detection_sidebar(None, cx);
        }
        self.sessions.remove_session(session_id);
        self.notify_task_sidebar(cx);
        self.refresh_app_menus(cx);
        cx.notify();
    }

    fn refresh_app_menus(&self, cx: &mut Context<Self>) {
        let sessions = self.sessions.sessions_snapshot();
        let editor_enabled = self.can_open_subtitle_editor();
        if cfg!(target_os = "macos") {
            menus::set_macos_menus(cx, &sessions, editor_enabled);
        } else {
            menus::set_app_menus(cx, &sessions, editor_enabled);
        }
        cx.notify();
    }

    fn sync_menu_refresh_listeners(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sessions = self.sessions.sessions_snapshot();
        let active_ids: HashSet<SessionId> = sessions.iter().map(|session| session.id).collect();
        self.menu_refresh_listeners
            .retain(|session_id, _| active_ids.contains(session_id));

        for session in &sessions {
            self.ensure_menu_refresh_listener(session, window, cx);
        }
    }

    fn ensure_menu_refresh_listener(
        &mut self,
        session: &VideoSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.menu_refresh_listeners.contains_key(&session.id) {
            return;
        }

        let handle = session.detection.clone();
        let main_handle = cx.entity().downgrade();
        let (notify_tx, mut notify_rx) = unbounded::<()>();

        let ui_task = window.spawn(cx, async move |cx| {
            while notify_rx.next().await.is_some() {
                let Some(main_window) = main_handle.upgrade() else {
                    break;
                };
                let _ = main_window.update(cx, |this, cx| {
                    this.refresh_app_menus(cx);
                });
            }
        });

        let (stop_tx, mut stop_rx) = oneshot::channel();
        let tokio_task = runtime::spawn(async move {
            let mut state_rx = handle.subscribe_state();
            let mut progress_rx = handle.subscribe_progress();
            let mut last_state = *state_rx.borrow();
            let mut last_completed = progress_rx.borrow().completed;

            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    changed = state_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let next_state = *state_rx.borrow();
                        if next_state != last_state {
                            last_state = next_state;
                            let _ = notify_tx.unbounded_send(());
                        }
                    }
                    changed = progress_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let completed = progress_rx.borrow().completed;
                        if completed != last_completed {
                            last_completed = completed;
                            let _ = notify_tx.unbounded_send(());
                        }
                    }
                }
            }
        });

        if tokio_task.is_none() {
            eprintln!("menu refresh listener failed: tokio runtime not initialized");
        }

        self.menu_refresh_listeners
            .insert(session.id, MenuRefreshListener::new(ui_task, stop_tx));
    }

    fn open_confirm_dialog(&mut self, config: ConfirmDialogConfig, cx: &mut Context<Self>) {
        let dialog = self.confirm_dialog.clone();
        dialog.update(cx, |dialog, cx| {
            dialog.open(config, cx);
        });
    }

    fn request_cancel_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.session(session_id) else {
            return;
        };

        let label = session.label.clone();
        let title_text: SharedString = "Stop task?".into();
        let message: SharedString =
            format!("Stop processing \"{label}\"? You can restart this task later.").into();
        let main_handle = cx.entity().downgrade();

        let title = ConfirmDialogTitle::element(move || {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_color(hsla(0.0, 0.0, 0.92, 1.0))
                .child(icon_sm(Icon::Stop, hsla(0.0, 0.0, 0.85, 1.0)))
                .child(title_text.clone())
                .into_any_element()
        });

        let cancel_button = ConfirmDialogButton::new(
            "Keep Running",
            ConfirmDialogButtonStyle::Secondary,
            true,
            Arc::new(|_, _| {}),
        );
        let confirm_button = ConfirmDialogButton::new(
            "Stop",
            ConfirmDialogButtonStyle::Danger,
            true,
            Arc::new(move |_window, cx| {
                if let Some(main_window) = main_handle.upgrade() {
                    main_window.update(cx, |this, cx| {
                        this.cancel_session(session_id, cx);
                    });
                }
            }),
        );

        self.open_confirm_dialog(
            ConfirmDialogConfig {
                title,
                message,
                buttons: vec![cancel_button, confirm_button],
                show_backdrop: true,
                backdrop_color: hsla(0.0, 0.0, 0.0, 0.55),
                close_on_outside: true,
            },
            cx,
        );
    }

    fn cancel_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        if let Some(session) = self.sessions.session(session_id) {
            session.detection.cancel();
        }
        self.notify_task_sidebar(cx);
    }

    pub(crate) fn request_remove_session(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.session(session_id) else {
            return;
        };

        let label = session.label.clone();
        let title_text: SharedString = "Remove task?".into();
        let message: SharedString =
            format!("Remove \"{label}\" from the task list? This will discard its results.").into();
        let main_handle = cx.entity().downgrade();

        let title = ConfirmDialogTitle::element(move || {
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_color(hsla(0.0, 0.0, 0.92, 1.0))
                .child(icon_sm(Icon::Trash, hsla(0.0, 0.0, 0.85, 1.0)))
                .child(title_text.clone())
                .into_any_element()
        });

        let cancel_button = ConfirmDialogButton::new(
            "Keep Task",
            ConfirmDialogButtonStyle::Secondary,
            true,
            Arc::new(|_, _| {}),
        );
        let confirm_button = ConfirmDialogButton::new(
            "Remove",
            ConfirmDialogButtonStyle::Danger,
            true,
            Arc::new(move |_window, cx| {
                if let Some(main_window) = main_handle.upgrade() {
                    main_window.update(cx, |this, cx| {
                        this.remove_session(session_id, cx);
                    });
                }
            }),
        );

        self.open_confirm_dialog(
            ConfirmDialogConfig {
                title,
                message,
                buttons: vec![cancel_button, confirm_button],
                show_backdrop: true,
                backdrop_color: hsla(0.0, 0.0, 0.0, 0.55),
                close_on_outside: true,
            },
            cx,
        );
    }

    /// Requests removal of the active session, or shows a warning if none is selected.
    pub(crate) fn request_remove_active_session(&mut self, cx: &mut Context<Self>) {
        if let Some(session_id) = self.active_session {
            self.request_remove_session(session_id, cx);
            return;
        }

        let ok_button = ConfirmDialogButton::new(
            "OK",
            ConfirmDialogButtonStyle::Primary,
            true,
            Arc::new(|_, _| {}),
        );
        self.open_confirm_dialog(
            ConfirmDialogConfig {
                title: ConfirmDialogTitle::text("No active task"),
                message: "Select a task before removing it.".into(),
                buttons: vec![ok_button],
                show_backdrop: true,
                backdrop_color: hsla(0.0, 0.0, 0.0, 0.55),
                close_on_outside: true,
            },
            cx,
        );
    }

    pub(crate) fn toggle_active_session_state(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.active_session else {
            return;
        };
        self.toggle_session_state(session_id, cx);
    }

    pub(crate) fn toggle_session_state(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.session(session_id) else {
            return;
        };
        let progress = session.detection.progress_snapshot();
        if progress.completed {
            return;
        }
        match session.detection.run_state() {
            DetectionRunState::Idle => {
                session.detection.start();
            }
            DetectionRunState::Running | DetectionRunState::Paused => {
                session.detection.toggle_pause();
            }
        }
        self.refresh_app_menus(cx);
    }

    fn save_active_session_state(&mut self, cx: &App) {
        let Some(session_id) = self.active_session else {
            return;
        };

        if let Some(info) = self.video_info.as_ref() {
            let snapshot = info.snapshot();
            self.sessions.update_playback(
                session_id,
                snapshot.last_timestamp,
                snapshot.last_frame_index,
            );
        }

        let luma_values = self.luma_handle.latest();

        let toolbar_state = self.toolbar_view.read(cx).snapshot();

        let roi = self.roi_handle.latest();

        self.sessions.update_settings(
            session_id,
            Some(luma_values.target),
            Some(luma_values.delta),
            Some(toolbar_state),
            Some(roi),
        );
    }

    fn load_session(&mut self, session: &VideoSession, cx: &mut Context<Self>) {
        let (player, controls, info) = VideoPlayer::new();
        let options = crate::gui::components::video_player::VideoOpenOptions {
            paused: true,
            start_frame: session.last_frame_index,
            backend: None,
        };
        controls.open_with(session.path.clone(), options);
        self.player = Some(cx.new(|_| player));
        self.controls = Some(controls.clone());
        self.video_info = Some(info.clone());
        self.replay_dismissed = false;
        self.set_replay_visible(false, cx);
        self.controls_view.update(cx, |controls_view, cx| {
            controls_view.set_handles(Some(controls.clone()), Some(info.clone()));
            cx.notify();
        });
        self.luma_controls_view.update(cx, |luma_controls, cx| {
            luma_controls.set_enabled(true, cx);
            if let (Some(target), Some(delta)) = (session.luma_target, session.luma_delta) {
                luma_controls.set_values(target, delta, cx);
            }
        });
        self.toolbar_view.update(cx, |toolbar_view, cx| {
            toolbar_view.set_controls(Some(controls), cx);
            if let Some(detector_kind) = resolve_overlay_detector_kind() {
                toolbar_view.set_detector_kind(detector_kind, cx);
            }
            if let Some(state) = session.toolbar_state {
                toolbar_view.restore(state, cx);
            }
            cx.notify();
        });
        self.roi_overlay.update(cx, |overlay, cx| {
            overlay.set_info_handle(Some(info), session.roi, cx);
        });
        cx.notify();
    }

    pub(crate) fn refresh_detector_backend(&mut self, cx: &mut Context<Self>) {
        let Some(detector_kind) = resolve_overlay_detector_kind() else {
            return;
        };
        self.toolbar_view.update(cx, |toolbar_view, cx| {
            toolbar_view.set_detector_kind(detector_kind, cx);
        });
    }

    fn release_player(&mut self, cx: &mut Context<Self>) {
        if let Some(controls) = self.controls.as_ref() {
            controls.shutdown();
        }
        self.player = None;
        self.controls = None;
        self.video_info = None;
        self.replay_dismissed = false;
        self.set_replay_visible(false, cx);
        self.controls_view.update(cx, |controls_view, cx| {
            controls_view.set_handles(None, None);
            cx.notify();
        });
        self.luma_controls_view.update(cx, |luma_controls, cx| {
            luma_controls.set_enabled(false, cx);
        });
        self.toolbar_view.update(cx, |toolbar_view, cx| {
            toolbar_view.set_controls(None, cx);
            cx.notify();
        });
        self.roi_overlay.update(cx, |overlay, cx| {
            overlay.set_info_handle(None, None, cx);
        });
        cx.notify();
    }

    fn build_detection_sidebar(
        &self,
        handle: DetectionHandle,
        cx: &mut Context<Self>,
    ) -> Entity<DetectionSidebar> {
        let detection_controls_view = cx.new(|_| DetectionControls::new(handle.clone()));
        let detection_metrics_view = cx.new(|_| DetectionMetrics::new(handle.clone()));
        let detection_subtitles_view =
            cx.new(|_| DetectedSubtitlesList::new(handle.clone(), self.controls.clone()));
        cx.new(|_| {
            DetectionSidebar::new(
                handle,
                detection_metrics_view.clone(),
                detection_controls_view.clone(),
                detection_subtitles_view.clone(),
            )
        })
    }

    fn update_detection_sidebar(
        &mut self,
        handle: Option<DetectionHandle>,
        cx: &mut Context<Self>,
    ) {
        let sidebar = handle.map(|handle| self.build_detection_sidebar(handle, cx));
        self.detection_sidebar_host.update(cx, |host, cx| {
            host.set_sidebar(sidebar, cx);
        });
    }

    fn notify_task_sidebar(&mut self, cx: &mut Context<Self>) {
        let task_sidebar = self.task_sidebar.clone();
        cx.defer(move |cx| {
            cx.update_entity(&task_sidebar, |_, cx| {
                cx.notify();
            });
        });
    }

    fn set_replay_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.replay_visible == visible {
            return;
        }
        self.replay_visible = visible;
        if let Some(player) = self.player.as_ref() {
            player.update(cx, |player, cx| {
                player.set_replay_blur_visible(visible, cx);
            });
        }
        cx.notify();
    }

    fn update_video_bounds(&mut self, bounds: Option<Bounds<Pixels>>) -> bool {
        if self.video_bounds != bounds {
            self.video_bounds = bounds;
            return true;
        }
        false
    }

    fn video_aspect(&self) -> Option<f32> {
        let info = self.video_info.as_ref()?;
        let snapshot = info.snapshot();
        let (width, height) = (snapshot.metadata.width?, snapshot.metadata.height?);
        if width == 0 || height == 0 {
            return None;
        }
        let aspect = width as f32 / height as f32;
        if !aspect.is_finite() || aspect <= 0.0 {
            return None;
        }
        Some(aspect)
    }

    fn video_frame_size(&self, total_height: f32) -> Option<(f32, f32)> {
        let bounds = self.video_bounds?;
        let container_w: f32 = bounds.size.width.into();
        if container_w <= 0.0 || total_height <= 0.0 {
            return None;
        }

        let width = container_w;
        let height = total_height * VIDEO_AREA_HEIGHT_RATIO;
        Some((width, height))
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let total_height: f32 = window.bounds().size.height.into();
        let video_content = self.player.clone();
        let video_aspect = self.video_aspect();
        let frame_size = self.video_frame_size(total_height);
        let ended = self
            .video_info
            .as_ref()
            .map(|info| {
                let snapshot = info.snapshot();
                snapshot.ended && snapshot.has_frame && !snapshot.scrubbing
            })
            .unwrap_or(false);
        if !ended {
            self.replay_dismissed = false;
        }
        self.set_replay_visible(ended && !self.replay_dismissed, cx);
        if cfg!(target_os = "macos") {
            self.sync_menu_refresh_listeners(window, cx);
        }

        let titlebar_children: Vec<AnyElement> = self
            .titlebar_actions
            .clone()
            .into_iter()
            .map(|actions| actions.into_any_element())
            .collect();
        self.titlebar.update(cx, move |titlebar, _| {
            titlebar.set_children(titlebar_children);
        });

        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .bg(rgb(0x1b1b1b))
            .child(
                div()
                    .flex_none()
                    .border_b(px(SIDEBAR_BORDER_WIDTH))
                    .border_color(rgb(SIDEBAR_BORDER_COLOR))
                    .child(self.titlebar.clone()),
            )
            .child({
                if cfg!(target_os = "macos") {
                    div()
                        .flex_none()
                        .h(px(0.0))
                        .px(px(0.0))
                        .border_b(px(0.0))
                        .border_color(rgb(SIDEBAR_BORDER_COLOR))
                } else {
                    div().h(px(0.0))
                }
            })
            .child({
                let mut video_frame = div()
                    .flex()
                    .rounded(px(16.0))
                    .overflow_hidden()
                    .bg(rgb(0x111111))
                    .items_center()
                    .justify_center()
                    .id(("video-frame", cx.entity_id()));
                if let Some((width, height)) = frame_size {
                    video_frame = video_frame.w(px(width)).h(px(height));
                } else {
                    video_frame = video_frame.w_full().h_full();
                }

                let frame_content = if let Some(video) = video_content {
                    let roi_overlay = self.roi_overlay.clone();
                    let replay_overlay = if self.replay_visible {
                        if let Some(controls) = self.controls.clone() {
                            let overlay_label = div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .text_xs()
                                .text_color(hsla(0.0, 0.0, 1.0, 0.85))
                                .child(icon_sm(Icon::RotateCcw, hsla(0.0, 0.0, 1.0, 0.85)))
                                .child("Replay");
                            Some(
                                div()
                                    .id(("replay-overlay", cx.entity_id()))
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(hsla(0.0, 0.0, 0.0, 0.18))
                                    .cursor_pointer()
                                    .child(overlay_label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        controls.replay();
                                        this.replay_dismissed = true;
                                        this.set_replay_visible(false, cx);
                                    })),
                            )
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let mut video_wrapper = div()
                        .relative()
                        .child(
                            div()
                                .relative()
                                .size_full()
                                .child(video)
                                .child(roi_overlay)
                                .children(replay_overlay),
                        )
                        .id(("video-wrapper", cx.entity_id()));

                    if let Some(aspect) = video_aspect {
                        let fit_by_height = frame_size
                            .map(|(width, height)| (width / height) >= aspect)
                            .unwrap_or(aspect < 1.0);
                        video_wrapper = video_wrapper.map(|mut view| {
                            view.style().aspect_ratio = Some(aspect);
                            view
                        });
                        video_wrapper = if fit_by_height {
                            video_wrapper.h_full()
                        } else {
                            video_wrapper.w_full()
                        };
                    } else {
                        video_wrapper = video_wrapper.w_full().h_full();
                    }

                    video_wrapper
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .size_full()
                        .cursor_pointer()
                        .text_color(hsla(0.0, 0.0, 1.0, 0.7))
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(6.0))
                                .child(icon_md(Icon::FileVideo, hsla(0.0, 0.0, 1.0, 0.7)))
                                .child("Click to select a video"),
                        )
                        .id(("video-wrapper", cx.entity_id()))
                        .on_click(cx.listener(|this, _event, window, cx| {
                            this.prompt_for_video(window, true, cx);
                        }))
                };
                let video_wrapper = video_frame.child(frame_content);

                let handle = cx.entity();
                let video_slot = div()
                    .flex()
                    .flex_none()
                    .w_full()
                    .on_children_prepainted(move |bounds, _window, cx| {
                        let bounds = bounds.first().copied();
                        handle.update(cx, |this, cx| {
                            if this.update_video_bounds(bounds) {
                                cx.notify();
                            }
                        });
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w_full()
                            .child(video_wrapper),
                    );

                let toolbar_video_group = div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(self.toolbar_view.clone())
                    .child(video_slot);

                let video_area = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .bg(rgb(0x1b1b1b))
                    .justify_start()
                    .px(px(8.0))
                    .pt(px(6.0))
                    .pb(px(2.0))
                    .gap(px(6.0))
                    .child(toolbar_video_group)
                    .child(self.controls_view.clone())
                    .child(self.luma_controls_view.clone());

                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .w_full()
                    .min_h(px(0.0))
                    .child(self.left_panel.clone())
                    .child(video_area)
                    .child(self.right_panel.clone())
            })
            .child(self.confirm_dialog.clone());

        root = root.on_action(cx.listener(|this, _: &OpenSubtitleEditor, window, cx| {
            this.open_subtitle_editor_window(window, cx);
        }));

        root
    }
}

fn is_supported_video_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    SUPPORTED_VIDEO_EXTENSIONS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(ext))
}

fn supported_video_extensions_detail() -> String {
    let list = SUPPORTED_VIDEO_EXTENSIONS
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Supported formats: {list}")
}
