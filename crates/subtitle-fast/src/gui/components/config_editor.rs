use std::fs;
use std::ops::Range;
use std::path::PathBuf;

use gpui::Styled;
use gpui::prelude::*;
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, InteractiveElement,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, Render, ScrollHandle, ShapedLine, SharedString, Style, TextRun, UTF16Selection,
    UnderlineStyle, Window, WindowBounds, WindowOptions, actions, div, fill, hsla, point, px,
    relative, rgb, rgba, size,
};

use crate::settings::{
    self, DecoderFileConfig, DetectionFileConfig, FileConfig, OcrFileConfig, OutputFileConfig,
    RoiFileConfig,
};

actions!(
    config_text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

const FIELD_LABEL_WIDTH: f32 = 200.0;
const INPUT_HEIGHT: f32 = 30.0;

pub struct ConfigWindow {
    fields: ConfigFields,
    scroll_handle: ScrollHandle,
    config_path: Option<PathBuf>,
    output_path: Option<PathBuf>,
    status: Option<StatusMessage>,
}

#[derive(Clone)]
struct StatusMessage {
    text: SharedString,
    is_error: bool,
}

impl ConfigWindow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let fields = ConfigFields::new(cx);
        let mut window = Self {
            fields,
            scroll_handle: ScrollHandle::new(),
            config_path: settings::default_config_path(),
            output_path: None,
            status: None,
        };
        window.load_from_disk(cx);
        window
    }

    pub fn open(cx: &mut App) -> gpui::WindowHandle<Self> {
        let bounds = Bounds::centered(None, size(px(540.0), px(640.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(520.0), px(520.0))),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("subtitle-fast settings".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| ConfigWindow::new(cx)),
        )
        .expect("settings window should open")
    }

    fn load_from_disk(&mut self, cx: &mut Context<Self>) {
        let Some(path) = settings::default_config_path() else {
            self.set_status("Config directory unavailable", true, cx);
            return;
        };

        self.config_path = Some(path.clone());
        self.output_path = None;

        let values = if path.exists() {
            match settings::load_file_config(&path) {
                Ok(file) => {
                    self.output_path = file.output.as_ref().and_then(|output| output.path.clone());
                    ConfigValues::from_file(file)
                }
                Err(err) => {
                    self.set_status(format!("Failed to read config: {err}"), true, cx);
                    ConfigValues::default_example()
                }
            }
        } else {
            self.set_status("Config file not found. Loaded defaults.", false, cx);
            ConfigValues::default_example()
        };

        self.fields.apply_values(values, cx);
    }

    fn save_to_disk(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.config_path.clone() else {
            self.set_status("Config path unavailable", true, cx);
            return;
        };

        let config = match self.build_config(cx) {
            Ok(config) => config,
            Err(err) => {
                self.set_status(err, true, cx);
                return;
            }
        };

        let Some(parent) = path.parent() else {
            self.set_status("Config path has no parent directory", true, cx);
            return;
        };

        if let Err(err) = fs::create_dir_all(parent) {
            self.set_status(format!("Failed to create config dir: {err}"), true, cx);
            return;
        }

        let toml = match toml::to_string_pretty(&config) {
            Ok(toml) => toml,
            Err(err) => {
                self.set_status(format!("Failed to serialize config: {err}"), true, cx);
                return;
            }
        };

        if let Err(err) = fs::write(&path, toml) {
            self.set_status(format!("Failed to write config: {err}"), true, cx);
            return;
        }

        self.set_status(
            format!("Saved configuration to {}", path.display()),
            false,
            cx,
        );
    }

    fn build_config(&self, cx: &App) -> Result<FileConfig, SharedString> {
        let values = self.fields.read_values(cx);

        let detection_sps = parse_required_u32("detection.samples_per_second", &values.sps)?;
        let detection_target = parse_required_u8("detection.target", &values.target)?;
        let detection_delta = parse_required_u8("detection.delta", &values.delta)?;

        let comparator = normalize_optional(&values.comparator);
        let roi = parse_roi_fields(
            &values.roi_x,
            &values.roi_y,
            &values.roi_width,
            &values.roi_height,
        )?;

        let decoder_backend = normalize_optional(&values.decoder_backend);
        let decoder_channel_capacity =
            parse_optional_usize("decoder.channel_capacity", &values.decoder_channel_capacity)?;

        let ocr_backend = normalize_optional(&values.ocr_backend);

        let detection = DetectionFileConfig {
            samples_per_second: Some(detection_sps),
            target: Some(detection_target),
            delta: Some(detection_delta),
            comparator,
            roi,
        };

        let decoder = if decoder_backend.is_some() || decoder_channel_capacity.is_some() {
            Some(DecoderFileConfig {
                backend: decoder_backend,
                channel_capacity: decoder_channel_capacity,
            })
        } else {
            None
        };

        let ocr = if ocr_backend.is_some() {
            Some(OcrFileConfig {
                backend: ocr_backend,
            })
        } else {
            None
        };

        Ok(FileConfig {
            detection: Some(detection),
            decoder,
            ocr,
            output: self.output_path.as_ref().map(|path| OutputFileConfig {
                path: Some(path.clone()),
            }),
        })
    }

    fn set_status(
        &mut self,
        text: impl Into<SharedString>,
        is_error: bool,
        cx: &mut Context<Self>,
    ) {
        self.status = Some(StatusMessage {
            text: text.into(),
            is_error,
        });
        cx.notify();
    }

    fn render_field(&self, label: &str, input: Entity<TextInput>) -> impl IntoElement {
        let label = div()
            .w(px(FIELD_LABEL_WIDTH))
            .text_size(px(12.0))
            .text_color(hsla(0.0, 0.0, 0.72, 1.0))
            .child(SharedString::from(label.to_string()));

        div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .w_full()
            .child(label)
            .child(div().flex_1().min_w(px(0.0)).child(input))
    }

    fn header_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let path_label = self
            .config_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "Unavailable".into());

        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(12.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(16.0))
                            .text_color(hsla(0.0, 0.0, 0.95, 1.0))
                            .child("Configuration"),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(hsla(0.0, 0.0, 0.6, 1.0))
                            .child(path_label),
                    ),
            )
            .child(self.action_buttons(cx))
    }

    fn action_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let reload = self.action_button(
            "Reload",
            ButtonStyle::Secondary,
            cx.listener(|this, _, _, cx| {
                this.load_from_disk(cx);
            }),
        );
        let save = self.action_button(
            "Save",
            ButtonStyle::Primary,
            cx.listener(|this, _, _, cx| {
                this.save_to_disk(cx);
            }),
        );

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(reload)
            .child(save)
    }

    fn status_row(&self) -> impl IntoElement {
        let Some(status) = self.status.as_ref() else {
            return div();
        };
        let color = if status.is_error {
            hsla(0.0, 0.75, 0.62, 1.0)
        } else {
            hsla(140.0, 0.55, 0.55, 1.0)
        };

        div()
            .text_size(px(11.0))
            .text_color(color)
            .child(status.text.clone())
    }

    fn fields_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let fields = div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(self.render_field("Detection samples/sec", self.fields.sps.clone()))
            .child(self.render_field("Detection target", self.fields.target.clone()))
            .child(self.render_field("Detection delta", self.fields.delta.clone()))
            .child(self.render_field("Detection comparator", self.fields.comparator.clone()))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(hsla(0.0, 0.0, 0.72, 1.0))
                    .child("Detection ROI (normalized 0-1)"),
            )
            .child(self.render_field("ROI X", self.fields.roi_x.clone()))
            .child(self.render_field("ROI Y", self.fields.roi_y.clone()))
            .child(self.render_field("ROI Width", self.fields.roi_width.clone()))
            .child(self.render_field("ROI Height", self.fields.roi_height.clone()))
            .child(self.render_field("Decoder backend", self.fields.decoder_backend.clone()))
            .child(self.render_field(
                "Decoder channel capacity",
                self.fields.decoder_channel_capacity.clone(),
            ))
            .child(self.render_field("OCR backend", self.fields.ocr_backend.clone()));

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .flex_1()
            .min_h(px(0.0))
            .id(("config-fields-scroll", cx.entity_id()))
            .overflow_y_scroll()
            .scrollbar_width(px(6.0))
            .track_scroll(&self.scroll_handle)
            .child(fields)
    }

    fn action_button(
        &self,
        label: impl Into<SharedString>,
        style: ButtonStyle,
        on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let (bg, hover, border, text) = match style {
            ButtonStyle::Primary => (
                hsla(0.0, 0.0, 0.92, 1.0),
                hsla(0.0, 0.0, 1.0, 1.0),
                hsla(0.0, 0.0, 0.8, 1.0),
                hsla(0.0, 0.0, 0.12, 1.0),
            ),
            ButtonStyle::Secondary => (
                hsla(0.0, 0.0, 0.18, 1.0),
                hsla(0.0, 0.0, 0.24, 1.0),
                hsla(0.0, 0.0, 0.3, 1.0),
                hsla(0.0, 0.0, 0.85, 1.0),
            ),
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(28.0))
            .px(px(14.0))
            .rounded(px(6.0))
            .bg(bg)
            .border_1()
            .border_color(border)
            .text_size(px(12.0))
            .text_color(text)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_mouse_down(MouseButton::Left, on_click)
            .child(label.into())
    }
}

impl Render for ConfigWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x151515))
            .p(px(20.0))
            .gap(px(16.0))
            .child(self.header_row(cx))
            .child(self.status_row())
            .child(self.fields_panel(cx))
    }
}

#[derive(Clone)]
struct ConfigValues {
    sps: SharedString,
    target: SharedString,
    delta: SharedString,
    comparator: SharedString,
    roi_x: SharedString,
    roi_y: SharedString,
    roi_width: SharedString,
    roi_height: SharedString,
    decoder_backend: SharedString,
    decoder_channel_capacity: SharedString,
    ocr_backend: SharedString,
}

impl ConfigValues {
    fn default_example() -> Self {
        Self {
            sps: "7".into(),
            target: "230".into(),
            delta: "12".into(),
            comparator: "".into(),
            roi_x: "".into(),
            roi_y: "".into(),
            roi_width: "".into(),
            roi_height: "".into(),
            decoder_backend: "".into(),
            decoder_channel_capacity: "".into(),
            ocr_backend: "".into(),
        }
    }

    fn from_file(file: FileConfig) -> Self {
        let mut values = Self::default_example();
        if let Some(det) = file.detection {
            if let Some(sps) = det.samples_per_second {
                values.sps = sps.to_string().into();
            }
            if let Some(target) = det.target {
                values.target = target.to_string().into();
            }
            if let Some(delta) = det.delta {
                values.delta = delta.to_string().into();
            }
            if let Some(comparator) = det.comparator {
                values.comparator = comparator.into();
            }
            if let Some(roi) = det.roi {
                if let Some(x) = roi.x {
                    values.roi_x = x.to_string().into();
                }
                if let Some(y) = roi.y {
                    values.roi_y = y.to_string().into();
                }
                if let Some(width) = roi.width {
                    values.roi_width = width.to_string().into();
                }
                if let Some(height) = roi.height {
                    values.roi_height = height.to_string().into();
                }
            }
        }

        if let Some(decoder) = file.decoder {
            if let Some(backend) = decoder.backend {
                values.decoder_backend = backend.into();
            }
            if let Some(capacity) = decoder.channel_capacity {
                values.decoder_channel_capacity = capacity.to_string().into();
            }
        }

        if let Some(ocr) = file.ocr {
            if let Some(backend) = ocr.backend {
                values.ocr_backend = backend.into();
            }
        }

        values
    }
}

struct ConfigFields {
    sps: Entity<TextInput>,
    target: Entity<TextInput>,
    delta: Entity<TextInput>,
    comparator: Entity<TextInput>,
    roi_x: Entity<TextInput>,
    roi_y: Entity<TextInput>,
    roi_width: Entity<TextInput>,
    roi_height: Entity<TextInput>,
    decoder_backend: Entity<TextInput>,
    decoder_channel_capacity: Entity<TextInput>,
    ocr_backend: Entity<TextInput>,
}

impl ConfigFields {
    fn new(cx: &mut Context<ConfigWindow>) -> Self {
        Self {
            sps: cx.new(|cx| TextInput::new(cx, "7")),
            target: cx.new(|cx| TextInput::new(cx, "230")),
            delta: cx.new(|cx| TextInput::new(cx, "12")),
            comparator: cx.new(|cx| TextInput::new(cx, "bitset-cover")),
            roi_x: cx.new(|cx| TextInput::new(cx, "0.0")),
            roi_y: cx.new(|cx| TextInput::new(cx, "0.0")),
            roi_width: cx.new(|cx| TextInput::new(cx, "1.0")),
            roi_height: cx.new(|cx| TextInput::new(cx, "1.0")),
            decoder_backend: cx.new(|cx| TextInput::new(cx, "dxva")),
            decoder_channel_capacity: cx.new(|cx| TextInput::new(cx, "32")),
            ocr_backend: cx.new(|cx| TextInput::new(cx, "auto")),
        }
    }

    fn apply_values(&self, values: ConfigValues, cx: &mut Context<ConfigWindow>) {
        let update =
            |input: &Entity<TextInput>, value: SharedString, cx: &mut Context<ConfigWindow>| {
                let _ = input.update(cx, |input, cx| input.set_text(value, cx));
            };

        update(&self.sps, values.sps, cx);
        update(&self.target, values.target, cx);
        update(&self.delta, values.delta, cx);
        update(&self.comparator, values.comparator, cx);
        update(&self.roi_x, values.roi_x, cx);
        update(&self.roi_y, values.roi_y, cx);
        update(&self.roi_width, values.roi_width, cx);
        update(&self.roi_height, values.roi_height, cx);
        update(&self.decoder_backend, values.decoder_backend, cx);
        update(
            &self.decoder_channel_capacity,
            values.decoder_channel_capacity,
            cx,
        );
        update(&self.ocr_backend, values.ocr_backend, cx);
    }

    fn read_values(&self, cx: &App) -> ConfigValues {
        let read = |input: &Entity<TextInput>, cx: &App| input.read(cx).text();
        ConfigValues {
            sps: read(&self.sps, cx),
            target: read(&self.target, cx),
            delta: read(&self.delta, cx),
            comparator: read(&self.comparator, cx),
            roi_x: read(&self.roi_x, cx),
            roi_y: read(&self.roi_y, cx),
            roi_width: read(&self.roi_width, cx),
            roi_height: read(&self.roi_height, cx),
            decoder_backend: read(&self.decoder_backend, cx),
            decoder_channel_capacity: read(&self.decoder_channel_capacity, cx),
            ocr_backend: read(&self.ocr_backend, cx),
        }
    }
}

fn normalize_optional(value: &SharedString) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_required_u32(label: &str, value: &SharedString) -> Result<u32, SharedString> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} is required").into());
    }
    let parsed: u32 = trimmed
        .parse()
        .map_err(|_| format!("{label} must be a number"))?;
    if parsed == 0 {
        return Err(format!("{label} must be at least 1").into());
    }
    Ok(parsed)
}

fn parse_required_u8(label: &str, value: &SharedString) -> Result<u8, SharedString> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} is required").into());
    }
    let parsed: u8 = trimmed
        .parse()
        .map_err(|_| format!("{label} must be 0-255"))?;
    Ok(parsed)
}

fn parse_optional_usize(label: &str, value: &SharedString) -> Result<Option<usize>, SharedString> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed: usize = trimmed
        .parse()
        .map_err(|_| format!("{label} must be a number"))?;
    if parsed == 0 {
        return Err(format!("{label} must be at least 1").into());
    }
    Ok(Some(parsed))
}

fn parse_roi_fields(
    x: &SharedString,
    y: &SharedString,
    width: &SharedString,
    height: &SharedString,
) -> Result<Option<RoiFileConfig>, SharedString> {
    let fields = [x, y, width, height];
    let any = fields.iter().any(|value| !value.trim().is_empty());
    if !any {
        return Ok(None);
    }

    let parse = |label: &str, value: &SharedString| -> Result<f32, SharedString> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(format!("ROI {label} is required").into());
        }
        trimmed
            .parse::<f32>()
            .map_err(|_| format!("ROI {label} must be a number").into())
    };

    Ok(Some(RoiFileConfig {
        x: Some(parse("x", x)?),
        y: Some(parse("y", y)?),
        width: Some(parse("width", width)?),
        height: Some(parse("height", height)?),
    }))
}

#[derive(Clone, Copy)]
enum ButtonStyle {
    Primary,
    Secondary,
}

struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl TextInput {
    fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::from(""),
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        }
    }

    fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let len = self.content.len();
        self.selected_range = len..len;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn text(&self) -> SharedString {
        self.content.clone()
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace("\n", " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .char_indices()
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = new_selected_range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .unwrap_or(range.start + new_text.len()..range.start + new_text.len());
        self.marked_range = new_selected_range.map(|range| self.range_from_utf16(&range));
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;

        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), hsla(0., 0., 0.65, 1.0))
        } else {
            (content, style.color)
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(cursor);
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(1.5), bounds.bottom() - bounds.top()),
                    ),
                    rgb(0x9ed4ff),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x2d78ff55),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection)
        }
        let line = prepaint.line.take().unwrap();
        line.paint(bounds.origin, window.line_height(), window, cx)
            .unwrap();

        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("ConfigTextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .line_height(px(INPUT_HEIGHT))
            .text_size(px(12.0))
            .text_color(hsla(0.0, 0.0, 0.9, 1.0))
            .child(
                div()
                    .h(px(INPUT_HEIGHT + 4.0))
                    .w_full()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(rgb(0x101010))
                    .border_1()
                    .border_color(rgb(0x2f2f2f))
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

pub fn bind_text_input_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, None),
        KeyBinding::new("delete", Delete, None),
        KeyBinding::new("left", Left, None),
        KeyBinding::new("right", Right, None),
        KeyBinding::new("shift-left", SelectLeft, None),
        KeyBinding::new("shift-right", SelectRight, None),
        KeyBinding::new("cmd-a", SelectAll, None),
        KeyBinding::new("cmd-v", Paste, None),
        KeyBinding::new("cmd-c", Copy, None),
        KeyBinding::new("cmd-x", Cut, None),
        KeyBinding::new("home", Home, None),
        KeyBinding::new("end", End, None),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, None),
    ]);
}
