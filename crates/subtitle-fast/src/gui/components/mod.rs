pub mod color_picker;
pub mod config_editor;
pub mod confirm_dialog;
pub mod detection_sidebar;
pub mod help_window;
pub mod menu_bar;
pub mod sidebar;
pub mod task_sidebar;
pub mod titlebar;
pub mod video_controls;
pub mod video_luma_controls;
pub mod video_player;
pub mod video_roi_overlay;
pub mod video_toolbar;

pub use color_picker::{ColorPicker, ColorPickerHandle};
pub use config_editor::{ConfigWindow, bind_text_input_keys};
pub use confirm_dialog::{
    ConfirmDialog, ConfirmDialogButton, ConfirmDialogButtonStyle, ConfirmDialogConfig,
    ConfirmDialogTitle,
};
pub use detection_sidebar::{
    DetectedSubtitlesList, DetectionControls, DetectionHandle, DetectionMetrics, DetectionRunState,
    DetectionSidebar, DetectionSidebarHost,
};
pub use help_window::HelpWindow;
pub use menu_bar::MenuBar;
pub use sidebar::{CollapseDirection, DragRange, DraggableEdge, Sidebar, SidebarHandle};
pub use task_sidebar::{TaskSidebar, TaskSidebarCallbacks};
pub use titlebar::Titlebar;
pub use video_controls::VideoControls;
pub use video_luma_controls::{VideoLumaControls, VideoLumaHandle, VideoLumaValues};
pub use video_player::{
    FramePreprocessor, Nv12FrameInfo, VideoPlayer, VideoPlayerControlHandle, VideoPlayerInfoHandle,
};
pub use video_roi_overlay::{VideoRoiHandle, VideoRoiOverlay};
pub use video_toolbar::{VideoToolbar, VideoToolbarState, VideoViewMode};
