use std::env;
use std::fs;
use std::ops::Range;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::Styled;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, Element,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable,
    GlobalElementId, InteractiveElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, PromptButton, PromptLevel, Render,
    ScrollHandle, ShapedLine, SharedString, Style, Subscription, TextRun, UTF16Selection,
    UnderlineStyle, Window, WindowBounds, WindowOptions, actions, div, fill, hsla, point, px,
    relative, rgb, rgba, size,
};

use crate::gui::components::Titlebar;
use crate::gui::icons::{Icon, icon_sm};
use crate::gui::menus;
use crate::settings::{
    self, DecoderFileConfig, DetectionFileConfig, FileConfig, OcrFileConfig, OutputFileConfig,
    RoiFileConfig,
};
use subtitle_fast_decoder::Configuration;

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
        Tab,
        TabPrev,
        Enter,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

const FIELD_LABEL_WIDTH: f32 = 200.0;
const INPUT_HEIGHT: f32 = 30.0;
const ERROR_ROW_HEIGHT: f32 = 14.0;
const ERROR_ROW_PADDING: f32 = 3.0;
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

pub struct ConfigWindow {
    fields: ConfigFields,
    scroll_handle: ScrollHandle,
    config_path: Option<PathBuf>,
    output_path: Option<PathBuf>,
    status: Option<StatusMessage>,
    field_errors: FieldErrors,
    autosave_enabled: bool,
    autosave_subscriptions: Vec<Subscription>,
    select_popup_bounds: Option<Bounds<Pixels>>,
    titlebar: Entity<Titlebar>,
    last_saved_values: ConfigValues,
    close_prompt_open: bool,
    allow_close: bool,
}

#[derive(Clone)]
struct StatusMessage {
    text: SharedString,
    is_error: bool,
}

#[derive(Clone, Default, PartialEq)]
struct FieldErrors {
    sps: Option<SharedString>,
    target: Option<SharedString>,
    delta: Option<SharedString>,
    roi_x: Option<SharedString>,
    roi_y: Option<SharedString>,
    roi_width: Option<SharedString>,
    roi_height: Option<SharedString>,
    decoder_channel_capacity: Option<SharedString>,
}

impl FieldErrors {
    fn is_clear(&self) -> bool {
        self.sps.is_none()
            && self.target.is_none()
            && self.delta.is_none()
            && self.roi_x.is_none()
            && self.roi_y.is_none()
            && self.roi_width.is_none()
            && self.roi_height.is_none()
            && self.decoder_channel_capacity.is_none()
    }
}

impl ConfigWindow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let fields = ConfigFields::new(cx);
        let titlebar = cx.new(|_| Titlebar::new("config-titlebar", "Settings"));
        let last_saved_values = ConfigValues::default_example();
        let mut window = Self {
            fields,
            scroll_handle: ScrollHandle::new(),
            config_path: settings::resolve_gui_config_path(),
            output_path: None,
            status: None,
            field_errors: FieldErrors::default(),
            autosave_enabled: false,
            autosave_subscriptions: Vec::new(),
            select_popup_bounds: None,
            titlebar,
            last_saved_values,
            close_prompt_open: false,
            allow_close: false,
        };
        window.load_from_disk(cx);
        window.autosave_enabled = true;
        window
    }

    pub fn open(cx: &mut App) -> gpui::WindowHandle<Self> {
        let bounds = Bounds::centered(None, size(px(820.0), px(560.0)), cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(820.0), px(560.0))),
                    is_resizable: false,
                    titlebar: Some(gpui::TitlebarOptions {
                        title: Some("subtitle-fast settings".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    window_decorations: Some(gpui::WindowDecorations::Client),
                    ..Default::default()
                },
                |_, cx| cx.new(ConfigWindow::new),
            )
            .expect("settings window should open");

        let _ = handle.update(cx, |this, window, cx| {
            this.register_close_prompt(window, cx);
            this.register_autosave(window, cx);
        });
        handle
    }

    fn load_from_disk(&mut self, cx: &mut Context<Self>) {
        let autosave_enabled = self.autosave_enabled;
        self.autosave_enabled = false;

        let Some(path) = settings::resolve_gui_config_path() else {
            self.set_status("Config path unavailable", true, cx);
            self.autosave_enabled = autosave_enabled;
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
            let defaults = ConfigValues::default_example();
            if self.write_defaults(&defaults, cx).is_err() {
                self.set_status("Failed to create config file.", true, cx);
            }
            defaults
        };

        self.fields.apply_values(values, cx);
        self.last_saved_values = self.fields.read_values(cx);
        self.apply_field_errors(FieldErrors::default(), cx);
        self.autosave_enabled = autosave_enabled;
    }

    fn save_to_disk(&mut self, cx: &mut Context<Self>) {
        let _ = self.save_to_disk_internal(true, cx);
    }

    fn build_config_from_values(values: &ConfigValues) -> Result<FileConfig, SharedString> {
        let detection_sps = parse_optional_u32("detection.samples_per_second", &values.sps)?;
        let detection_target = parse_optional_u8("detection.target", &values.target)?;
        let detection_delta = parse_optional_u8("detection.delta", &values.delta)?;

        let detector = normalize_optional(&values.detector_backend);
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

        let detection = if detection_sps.is_some()
            || detection_target.is_some()
            || detection_delta.is_some()
            || detector.is_some()
            || comparator.is_some()
            || roi.is_some()
        {
            Some(DetectionFileConfig {
                samples_per_second: detection_sps,
                target: detection_target,
                delta: detection_delta,
                detector,
                comparator,
                roi,
            })
        } else {
            None
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
            detection,
            decoder,
            ocr,
            output: None,
        })
    }

    fn save_to_disk_internal(
        &mut self,
        show_status: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), SharedString> {
        if !self.validate_fields(cx) {
            return Err("Invalid configuration values".into());
        }
        let Some(path) = self.config_path.clone() else {
            let message: SharedString = "Config path unavailable".into();
            if show_status {
                self.set_status(message.clone(), true, cx);
            }
            return Err(message);
        };

        let values = self.fields.read_values(cx);
        let detector_changed = values.detector_backend != self.last_saved_values.detector_backend;
        let mut config = Self::build_config_from_values(&values)?;
        config.output = self.output_path.as_ref().map(|path| OutputFileConfig {
            path: Some(path.clone()),
        });

        let Some(parent) = path.parent() else {
            let message: SharedString = "Config path has no parent directory".into();
            if show_status {
                self.set_status(message.clone(), true, cx);
            }
            return Err(message);
        };

        if let Err(err) = fs::create_dir_all(parent) {
            let message: SharedString = format!("Failed to create config dir: {err}").into();
            if show_status {
                self.set_status(message.clone(), true, cx);
            }
            return Err(message);
        }

        let toml = toml::to_string_pretty(&config).map_err(|err| {
            let message: SharedString = format!("Failed to serialize config: {err}").into();
            if show_status {
                self.set_status(message.clone(), true, cx);
            }
            message
        })?;

        if let Err(err) = fs::write(&path, toml) {
            let message: SharedString = format!("Failed to write config: {err}").into();
            if show_status {
                self.set_status(message.clone(), true, cx);
            }
            return Err(message);
        }

        self.last_saved_values = values;
        if detector_changed {
            self.notify_detector_backend_change(cx);
        }
        if self.status.is_some() {
            self.status = None;
            cx.notify();
        }
        Ok(())
    }

    fn notify_detector_backend_change(&self, cx: &mut Context<Self>) {
        cx.defer(|cx| {
            let Some(window) = menus::main_window_handle(cx) else {
                return;
            };
            let _ = window.update(cx, |main_window, _window, cx| {
                main_window.refresh_detector_backend(cx);
            });
        });
    }

    fn write_defaults(
        &mut self,
        values: &ConfigValues,
        cx: &mut Context<Self>,
    ) -> Result<(), SharedString> {
        self.fields.apply_values(values.clone(), cx);
        self.save_to_disk_internal(false, cx)
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

    fn register_autosave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut subscriptions = Vec::new();
        let text_inputs = [
            self.fields.sps.clone(),
            self.fields.target.clone(),
            self.fields.delta.clone(),
            self.fields.roi_x.clone(),
            self.fields.roi_y.clone(),
            self.fields.roi_width.clone(),
            self.fields.roi_height.clone(),
            self.fields.decoder_channel_capacity.clone(),
        ];
        for input in text_inputs {
            subscriptions.push(cx.observe(&input, |this, _input, cx| {
                this.handle_field_change(cx);
            }));
            let focus_handle = input.read(cx).focus_handle.clone();
            subscriptions.push(cx.on_blur(&focus_handle, window, |this, _window, cx| {
                this.handle_autosave(cx);
            }));
        }
        subscriptions.push(cx.observe(&self.fields.comparator, |this, _input, cx| {
            this.handle_autosave(cx);
        }));
        subscriptions.push(
            cx.observe(&self.fields.detector_backend, |this, _input, cx| {
                this.handle_autosave(cx);
            }),
        );
        subscriptions.push(
            cx.observe(&self.fields.decoder_backend, |this, _input, cx| {
                this.handle_autosave(cx);
            }),
        );
        subscriptions.push(cx.observe(&self.fields.ocr_backend, |this, _input, cx| {
            this.handle_autosave(cx);
        }));

        self.autosave_subscriptions = subscriptions;
    }

    fn handle_field_change(&mut self, cx: &mut Context<Self>) {
        if !self.autosave_enabled {
            return;
        }
        self.validate_fields(cx);
    }

    fn handle_autosave(&mut self, cx: &mut Context<Self>) {
        if !self.autosave_enabled {
            return;
        }
        let valid = self.validate_fields(cx);
        if !valid || !self.is_dirty(cx) {
            return;
        }
        let _ = self.save_to_disk_internal(false, cx);
    }

    fn set_input_invalid(&self, input: &Entity<TextInput>, invalid: bool, cx: &mut Context<Self>) {
        input.update(cx, |input, cx| {
            input.set_invalid(invalid, cx);
        });
    }

    fn apply_field_errors(&mut self, errors: FieldErrors, cx: &mut Context<Self>) {
        let changed = self.field_errors != errors;
        self.field_errors = errors;
        self.set_input_invalid(&self.fields.sps, self.field_errors.sps.is_some(), cx);
        self.set_input_invalid(&self.fields.target, self.field_errors.target.is_some(), cx);
        self.set_input_invalid(&self.fields.delta, self.field_errors.delta.is_some(), cx);
        self.set_input_invalid(&self.fields.roi_x, self.field_errors.roi_x.is_some(), cx);
        self.set_input_invalid(&self.fields.roi_y, self.field_errors.roi_y.is_some(), cx);
        self.set_input_invalid(
            &self.fields.roi_width,
            self.field_errors.roi_width.is_some(),
            cx,
        );
        self.set_input_invalid(
            &self.fields.roi_height,
            self.field_errors.roi_height.is_some(),
            cx,
        );
        self.set_input_invalid(
            &self.fields.decoder_channel_capacity,
            self.field_errors.decoder_channel_capacity.is_some(),
            cx,
        );
        if changed {
            cx.notify();
        }
    }

    fn validate_fields(&mut self, cx: &mut Context<Self>) -> bool {
        let values = self.fields.read_values(cx);
        let errors = FieldErrors {
            sps: validate_optional_u32("Detection samples/sec", &values.sps),
            target: validate_optional_u8("Detection target", &values.target),
            delta: validate_optional_u8("Detection delta", &values.delta),
            roi_x: validate_roi_value("ROI X", &values.roi_x),
            roi_y: validate_roi_value("ROI Y", &values.roi_y),
            roi_width: validate_roi_value("ROI Width", &values.roi_width),
            roi_height: validate_roi_value("ROI Height", &values.roi_height),
            decoder_channel_capacity: validate_optional_usize(
                "Decoder channel capacity",
                &values.decoder_channel_capacity,
            ),
        };

        self.apply_field_errors(errors, cx);
        self.field_errors.is_clear()
    }

    fn close_open_selects(&mut self, cx: &mut Context<Self>) {
        let selects = [
            self.fields.comparator.clone(),
            self.fields.detector_backend.clone(),
            self.fields.decoder_backend.clone(),
            self.fields.ocr_backend.clone(),
        ];
        for select in selects {
            select.update(cx, |select, cx| {
                select.close(cx);
            });
        }
        self.select_popup_bounds = None;
    }

    fn has_open_select(&self, cx: &Context<Self>) -> bool {
        let selects = [
            self.fields.comparator.clone(),
            self.fields.detector_backend.clone(),
            self.fields.decoder_backend.clone(),
            self.fields.ocr_backend.clone(),
        ];
        selects.into_iter().any(|select| select.read(cx).open)
    }

    fn select_popup(&mut self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.select_popup_bounds = None;
        let selects = [
            self.fields.comparator.clone(),
            self.fields.detector_backend.clone(),
            self.fields.decoder_backend.clone(),
            self.fields.ocr_backend.clone(),
        ];
        for select_entity in selects {
            let (bounds, options, selected) = {
                let select = select_entity.read(cx);
                if !select.open {
                    continue;
                }
                let bounds = select.button_bounds?;
                let options = select.options.clone();
                let selected = select.selected;
                (bounds, options, selected)
            };

            let options = options
                .into_iter()
                .enumerate()
                .map(|(index, option)| {
                    let is_selected = index == selected;
                    let row_bg = if is_selected {
                        rgb(0x2b2b2b)
                    } else {
                        rgb(0x202020)
                    };
                    let select_handle = select_entity.clone();
                    div()
                        .flex()
                        .items_center()
                        .h(px(26.0))
                        .px(px(10.0))
                        .bg(row_bg)
                        .text_size(px(12.0))
                        .text_color(hsla(0.0, 0.0, 0.9, 1.0))
                        .hover(move |style| style.bg(rgb(0x262626)))
                        .cursor_pointer()
                        .child(option.label)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |_, _event, _window, cx| {
                                select_handle.update(cx, |select, cx| {
                                    select.select(index, cx);
                                });
                            }),
                        )
                })
                .collect::<Vec<_>>();

            let popup_offset = 4.0;
            let row_height = 26.0;
            let divider_height = 1.0;
            let popup_height = options.len() as f32 * row_height + divider_height;
            let window_bounds = Bounds {
                origin: point(px(0.0), px(0.0)),
                size: window.viewport_size(),
            };
            let below_space: f32 = (window_bounds.bottom() - bounds.bottom()).into();
            let above_space: f32 = (bounds.top() - window_bounds.top()).into();
            let show_above = below_space < popup_height + popup_offset
                && above_space >= popup_height + popup_offset;
            let popup_top = if show_above {
                bounds.top() - px(popup_height + popup_offset)
            } else {
                bounds.bottom() + px(popup_offset)
            };

            let popup = div()
                .absolute()
                .left(bounds.left())
                .top(popup_top)
                .w(bounds.size.width)
                .flex()
                .flex_col()
                .bg(rgb(0x202020))
                .border_1()
                .border_color(rgb(0x2b2b2b))
                .rounded(px(6.0))
                .shadow(vec![gpui::BoxShadow {
                    color: hsla(0.0, 0.0, 0.0, 0.35),
                    offset: gpui::point(px(0.0), px(4.0)),
                    blur_radius: px(8.0),
                    spread_radius: px(0.0),
                }])
                .occlude()
                .child(div().h(px(1.0)).bg(rgb(0x2b2b2b)))
                .children(options);

            let handle = cx.entity();
            let popup_wrapper = div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .on_children_prepainted(move |bounds, _window, cx| {
                    let bounds = bounds.first().copied();
                    handle.update(cx, |this, _| {
                        this.select_popup_bounds = bounds;
                    });
                });

            return Some(popup_wrapper.child(popup).into_any_element());
        }
        None
    }

    fn render_field(
        &self,
        label: &str,
        input: Entity<TextInput>,
        error: Option<SharedString>,
    ) -> impl IntoElement {
        let label = div()
            .w(px(FIELD_LABEL_WIDTH))
            .text_size(px(12.0))
            .text_color(hsla(0.0, 0.0, 0.72, 1.0))
            .child(SharedString::from(label.to_string()));

        let row = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .w_full()
            .child(label)
            .child(div().flex_1().min_w(px(0.0)).child(input));

        let error_row = div()
            .flex()
            .items_start()
            .gap(px(12.0))
            .w_full()
            .pb(px(ERROR_ROW_PADDING))
            .child(div().w(px(FIELD_LABEL_WIDTH)))
            .child(
                div()
                    .flex_1()
                    .h(px(ERROR_ROW_HEIGHT))
                    .text_size(px(11.0))
                    .text_color(hsla(0.0, 0.7, 0.6, 1.0))
                    .text_ellipsis()
                    .child(error.unwrap_or_else(|| "".into())),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(row)
            .child(error_row)
    }

    fn render_select_field(&self, label: &str, input: Entity<SelectInput>) -> impl IntoElement {
        let label = div()
            .w(px(FIELD_LABEL_WIDTH))
            .text_size(px(12.0))
            .text_color(hsla(0.0, 0.0, 0.72, 1.0))
            .child(SharedString::from(label.to_string()));

        let row = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .w_full()
            .child(label)
            .child(div().flex_1().min_w(px(0.0)).child(input));

        let error_row = div()
            .flex()
            .items_start()
            .gap(px(12.0))
            .w_full()
            .pb(px(ERROR_ROW_PADDING))
            .child(div().w(px(FIELD_LABEL_WIDTH)))
            .child(div().flex_1().h(px(ERROR_ROW_HEIGHT)));

        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(row)
            .child(error_row)
    }

    fn header_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(12.0))
            .child(
                div().flex().flex_col().gap(px(0.0)).child(
                    div()
                        .text_size(px(16.0))
                        .text_color(hsla(0.0, 0.0, 0.95, 1.0))
                        .child("Configuration"),
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
            .gap(px(0.0))
            .child(self.render_field(
                "Detection samples/sec",
                self.fields.sps.clone(),
                self.field_errors.sps.clone(),
            ))
            .child(self.render_field(
                "Detection target",
                self.fields.target.clone(),
                self.field_errors.target.clone(),
            ))
            .child(self.render_field(
                "Detection delta",
                self.fields.delta.clone(),
                self.field_errors.delta.clone(),
            ))
            .child(
                self.render_select_field("Detection backend", self.fields.detector_backend.clone()),
            )
            .child(self.render_select_field("Detection comparator", self.fields.comparator.clone()))
            .child(self.render_roi_row())
            .child(self.render_select_field("Decoder backend", self.fields.decoder_backend.clone()))
            .child(self.render_field(
                "Decoder channel capacity",
                self.fields.decoder_channel_capacity.clone(),
                self.field_errors.decoder_channel_capacity.clone(),
            ))
            .child(self.render_select_field("OCR backend", self.fields.ocr_backend.clone()));

        div()
            .flex()
            .flex_col()
            .gap(px(0.0))
            .flex_1()
            .min_h(px(0.0))
            .id(("config-fields-scroll", cx.entity_id()))
            .overflow_y_scroll()
            .scrollbar_width(px(6.0))
            .track_scroll(&self.scroll_handle)
            .child(fields)
    }

    fn render_roi_row(&self) -> impl IntoElement {
        let label = div()
            .w(px(FIELD_LABEL_WIDTH))
            .text_size(px(12.0))
            .text_color(hsla(0.0, 0.0, 0.72, 1.0))
            .child("Detection ROI (x, y, w, h)");

        let input_row = div()
            .flex()
            .items_center()
            .gap(px(12.0))
            .w_full()
            .child(label)
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(self.fields.roi_x.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(self.fields.roi_y.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(self.fields.roi_width.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(self.fields.roi_height.clone()),
            );

        let error_row = div()
            .flex()
            .items_start()
            .gap(px(12.0))
            .w_full()
            .pb(px(ERROR_ROW_PADDING))
            .child(div().w(px(FIELD_LABEL_WIDTH)))
            .child(
                div()
                    .flex_1()
                    .h(px(ERROR_ROW_HEIGHT))
                    .text_size(px(11.0))
                    .text_color(hsla(0.0, 0.7, 0.6, 1.0))
                    .text_ellipsis()
                    .child(self.field_errors.roi_x.clone().unwrap_or_else(|| "".into())),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(ERROR_ROW_HEIGHT))
                    .text_size(px(11.0))
                    .text_color(hsla(0.0, 0.7, 0.6, 1.0))
                    .text_ellipsis()
                    .child(self.field_errors.roi_y.clone().unwrap_or_else(|| "".into())),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(ERROR_ROW_HEIGHT))
                    .text_size(px(11.0))
                    .text_color(hsla(0.0, 0.7, 0.6, 1.0))
                    .text_ellipsis()
                    .child(
                        self.field_errors
                            .roi_width
                            .clone()
                            .unwrap_or_else(|| "".into()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(ERROR_ROW_HEIGHT))
                    .text_size(px(11.0))
                    .text_color(hsla(0.0, 0.7, 0.6, 1.0))
                    .text_ellipsis()
                    .child(
                        self.field_errors
                            .roi_height
                            .clone()
                            .unwrap_or_else(|| "".into()),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(input_row)
            .child(error_row)
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

    fn is_dirty(&self, cx: &App) -> bool {
        self.fields.read_values(cx) != self.last_saved_values
    }

    fn register_close_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        window.on_window_should_close(cx, move |window, cx| {
            let mut should_close = true;
            entity.update(cx, |this, cx| {
                if this.allow_close {
                    should_close = true;
                    return;
                }

                if !this.is_dirty(cx) {
                    should_close = true;
                    return;
                }

                should_close = false;
                if this.close_prompt_open {
                    return;
                }

                this.close_prompt_open = true;
                let weak = cx.entity().downgrade();
                let task = window.spawn(cx, move |cx: &mut gpui::AsyncWindowContext| {
                    let mut cx = cx.clone();
                    async move {
                        let buttons = [
                            PromptButton::new("Save"),
                            PromptButton::new("Discard"),
                            PromptButton::cancel("Cancel"),
                        ];
                        let receiver = cx.prompt(
                            PromptLevel::Warning,
                            "Unsaved changes",
                            Some("Save your changes before closing?"),
                            &buttons,
                        );
                        let selection: Option<usize> = receiver.await.ok();
                        let mut allow_close = false;
                        if let Some(selection) = selection {
                            match selection {
                                0 => {
                                    let saved = weak
                                        .update(&mut cx, |this, cx| {
                                            this.save_to_disk_internal(true, cx).is_ok()
                                        })
                                        .unwrap_or(false);
                                    allow_close = saved;
                                }
                                1 => {
                                    allow_close = true;
                                }
                                _ => {}
                            }
                        }

                        let _ = weak.update(&mut cx, |this, _| {
                            this.close_prompt_open = false;
                            if allow_close {
                                this.allow_close = true;
                            }
                        });

                        if allow_close {
                            let _ = cx.update(|window: &mut Window, _| window.remove_window());
                        }
                    }
                });
                task.detach();
            });
            should_close
        });
    }
}

impl Render for ConfigWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_open = self.has_open_select(cx);
        let popup = self.select_popup(window, cx);
        if has_open {
            let handle = cx.entity();
            window.on_mouse_event(move |event: &MouseDownEvent, phase, _window, cx| {
                if phase != DispatchPhase::Capture {
                    return;
                }
                if event.button != MouseButton::Left {
                    return;
                }
                let position = event.position;
                handle.update(cx, |this, cx| {
                    if !this.has_open_select(cx) {
                        return;
                    }
                    let mut inside = false;
                    if let Some(bounds) = this.select_popup_bounds
                        && bounds.contains(&position)
                    {
                        inside = true;
                    }
                    if !inside {
                        let selects = [
                            this.fields.comparator.clone(),
                            this.fields.detector_backend.clone(),
                            this.fields.decoder_backend.clone(),
                            this.fields.ocr_backend.clone(),
                        ];
                        for select in selects {
                            let select = select.read(cx);
                            if !select.open {
                                continue;
                            }
                            if let Some(bounds) = select.button_bounds
                                && bounds.contains(&position)
                            {
                                inside = true;
                                break;
                            }
                        }
                    }
                    if !inside {
                        this.close_open_selects(cx);
                    }
                });
            });
        }
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.0))
            .bg(rgb(0x1b1b1b))
            .child(
                div()
                    .flex_none()
                    .border_b(px(1.0))
                    .border_color(rgb(0x2b2b2b))
                    .child(self.titlebar.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(px(16.0))
                    .px(px(20.0))
                    .py(px(18.0))
                    .child(self.header_row(cx))
                    .child(self.status_row())
                    .child(self.fields_panel(cx)),
            );
        if let Some(popup) = popup {
            root = root.child(popup);
        }
        root
    }
}

#[derive(Clone, PartialEq)]
struct ConfigValues {
    sps: SharedString,
    target: SharedString,
    delta: SharedString,
    detector_backend: SharedString,
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
            detector_backend: "".into(),
            comparator: "".into(),
            roi_x: "0.15".into(),
            roi_y: "0.8".into(),
            roi_width: "0.7".into(),
            roi_height: "0.2".into(),
            decoder_backend: "".into(),
            decoder_channel_capacity: "".into(),
            ocr_backend: "auto".into(),
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
            if let Some(detector) = det.detector {
                values.detector_backend = detector.into();
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

        if let Some(ocr) = file.ocr
            && let Some(backend) = ocr.backend
        {
            values.ocr_backend = backend.into();
        }

        values
    }
}

struct ConfigFields {
    sps: Entity<TextInput>,
    target: Entity<TextInput>,
    delta: Entity<TextInput>,
    detector_backend: Entity<SelectInput>,
    comparator: Entity<SelectInput>,
    roi_x: Entity<TextInput>,
    roi_y: Entity<TextInput>,
    roi_width: Entity<TextInput>,
    roi_height: Entity<TextInput>,
    decoder_backend: Entity<SelectInput>,
    decoder_channel_capacity: Entity<TextInput>,
    ocr_backend: Entity<SelectInput>,
}

impl ConfigFields {
    fn new(cx: &mut Context<ConfigWindow>) -> Self {
        let comparator_options = vec![
            SelectOption::new("auto", ""),
            SelectOption::new("bitset-cover", "bitset-cover"),
            SelectOption::new("sparse-chamfer", "sparse-chamfer"),
        ];
        let detector_backend_options = detector_backend_options();
        let decoder_backend_options = decoder_backend_options();
        let ocr_backend_options = ocr_backend_options();

        Self {
            sps: cx.new(|cx| TextInput::new(cx, "7", InputKind::Integer)),
            target: cx.new(|cx| TextInput::new(cx, "230", InputKind::Integer)),
            delta: cx.new(|cx| TextInput::new(cx, "12", InputKind::Integer)),
            detector_backend: cx.new(|_| SelectInput::new(detector_backend_options, "")),
            comparator: cx.new(|_| SelectInput::new(comparator_options, "")),
            roi_x: cx.new(|cx| TextInput::new(cx, "0.15", InputKind::Float)),
            roi_y: cx.new(|cx| TextInput::new(cx, "0.8", InputKind::Float)),
            roi_width: cx.new(|cx| TextInput::new(cx, "0.7", InputKind::Float)),
            roi_height: cx.new(|cx| TextInput::new(cx, "0.2", InputKind::Float)),
            decoder_backend: cx.new(|_| SelectInput::new(decoder_backend_options, "")),
            decoder_channel_capacity: cx.new(|cx| TextInput::new(cx, "32", InputKind::Integer)),
            ocr_backend: cx.new(|_| SelectInput::new(ocr_backend_options, "auto")),
        }
    }

    fn apply_values(&self, values: ConfigValues, cx: &mut Context<ConfigWindow>) {
        let update =
            |input: &Entity<TextInput>, value: SharedString, cx: &mut Context<ConfigWindow>| {
                input.update(cx, |input, cx| input.set_text(value, cx));
            };
        let update_select =
            |input: &Entity<SelectInput>, value: SharedString, cx: &mut Context<ConfigWindow>| {
                input.update(cx, |input, cx| {
                    input.set_value(value, cx);
                });
            };

        update(&self.sps, values.sps, cx);
        update(&self.target, values.target, cx);
        update(&self.delta, values.delta, cx);
        update_select(&self.detector_backend, values.detector_backend, cx);
        update_select(&self.comparator, values.comparator, cx);
        update(&self.roi_x, values.roi_x, cx);
        update(&self.roi_y, values.roi_y, cx);
        update(&self.roi_width, values.roi_width, cx);
        update(&self.roi_height, values.roi_height, cx);
        update_select(&self.decoder_backend, values.decoder_backend, cx);
        update(
            &self.decoder_channel_capacity,
            values.decoder_channel_capacity,
            cx,
        );
        update_select(&self.ocr_backend, values.ocr_backend, cx);
    }

    fn read_values(&self, cx: &App) -> ConfigValues {
        let read = |input: &Entity<TextInput>, cx: &App| input.read(cx).text();
        let read_select = |input: &Entity<SelectInput>, cx: &App| input.read(cx).value();
        ConfigValues {
            sps: read(&self.sps, cx),
            target: read(&self.target, cx),
            delta: read(&self.delta, cx),
            detector_backend: read_select(&self.detector_backend, cx),
            comparator: read_select(&self.comparator, cx),
            roi_x: read(&self.roi_x, cx),
            roi_y: read(&self.roi_y, cx),
            roi_width: read(&self.roi_width, cx),
            roi_height: read(&self.roi_height, cx),
            decoder_backend: read_select(&self.decoder_backend, cx),
            decoder_channel_capacity: read(&self.decoder_channel_capacity, cx),
            ocr_backend: read_select(&self.ocr_backend, cx),
        }
    }
}

fn decoder_backend_options() -> Vec<SelectOption> {
    let mut options = vec![SelectOption::new("Default", "")];
    let available = Configuration::available_backends();
    for backend in available {
        let name = backend.as_str();
        if name == "mock" && !github_ci_active() {
            continue;
        }
        options.push(SelectOption::new(name, name));
    }
    options
}

fn detector_backend_options() -> Vec<SelectOption> {
    let mut options = vec![SelectOption::new("Default", "")];
    options.push(SelectOption::new("auto", "auto"));
    options.push(SelectOption::new("projection-band", "projection-band"));
    options.push(SelectOption::new("integral-band", "integral-band"));
    #[cfg(all(feature = "detector-vision", target_os = "macos"))]
    {
        options.push(SelectOption::new("macos-vision", "macos-vision"));
    }
    options
}

fn ocr_backend_options() -> Vec<SelectOption> {
    let mut options = vec![SelectOption::new("auto", "auto")];
    #[cfg(all(feature = "ocr-vision", target_os = "macos"))]
    {
        options.push(SelectOption::new("vision", "vision"));
    }
    #[cfg(feature = "ocr-ort")]
    {
        options.push(SelectOption::new("ort", "ort"));
    }
    options.push(SelectOption::new("noop", "noop"));
    options
}

fn github_ci_active() -> bool {
    env::var("GITHUB_ACTIONS")
        .map(|value| !value.is_empty() && value != "false")
        .unwrap_or(false)
}

#[derive(Clone)]
struct SelectOption {
    label: SharedString,
    value: SharedString,
}

impl SelectOption {
    fn new(label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }
}

struct SelectInput {
    options: Vec<SelectOption>,
    selected: usize,
    open: bool,
    button_bounds: Option<Bounds<Pixels>>,
}

impl SelectInput {
    fn new(options: Vec<SelectOption>, default_value: &str) -> Self {
        let selected = options
            .iter()
            .position(|option| option.value.as_ref() == default_value)
            .unwrap_or(0);
        Self {
            options,
            selected,
            open: false,
            button_bounds: None,
        }
    }

    fn set_value(&mut self, value: SharedString, cx: &mut Context<Self>) {
        if let Some(index) = self.options.iter().position(|option| option.value == value)
            && self.selected != index
        {
            self.selected = index;
            cx.notify();
        }
    }

    fn value(&self) -> SharedString {
        self.options
            .get(self.selected)
            .map(|option| option.value.clone())
            .unwrap_or_default()
    }

    fn toggle_open(&mut self, cx: &mut Context<Self>) {
        self.open = !self.open;
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.selected != index {
            self.selected = index;
        }
        self.open = false;
        cx.notify();
    }
}

impl Render for SelectInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border_color = rgb(0x2b2b2b);
        let background = rgb(0x202020);
        let hover_bg = rgb(0x262626);
        let text_color = hsla(0.0, 0.0, 0.9, 1.0);
        let muted_color = hsla(0.0, 0.0, 0.65, 1.0);

        let current = self
            .options
            .get(self.selected)
            .map(|option| option.label.clone())
            .unwrap_or_else(|| SharedString::from("Select"));

        let selector = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(INPUT_HEIGHT))
            .px(px(10.0))
            .w_full()
            .rounded(px(6.0))
            .bg(background)
            .border_1()
            .border_color(border_color)
            .text_size(px(12.0))
            .text_color(text_color)
            .cursor_pointer()
            .hover(move |style| style.bg(hover_bg))
            .child(current)
            .child(icon_sm(Icon::ChevronDown, muted_color))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.toggle_open(cx);
                }),
            );

        let handle = cx.entity();
        let selector_wrapper = div()
            .on_children_prepainted(move |bounds, _window, cx| {
                let bounds = bounds.first().copied();
                handle.update(cx, |this, _cx| {
                    this.button_bounds = bounds;
                });
            })
            .child(selector);

        div().relative().w_full().child(selector_wrapper)
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

fn validate_optional_u32(label: &str, value: &SharedString) -> Option<SharedString> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<u32>() {
        Ok(parsed) if parsed >= 1 => None,
        Ok(_) => Some(format!("{label} must be at least 1").into()),
        Err(_) => Some(format!("{label} must be a number").into()),
    }
}

fn validate_optional_u8(label: &str, value: &SharedString) -> Option<SharedString> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<u8>() {
        Ok(_) => None,
        Err(_) => Some(format!("{label} must be 0-255").into()),
    }
}

fn validate_optional_usize(label: &str, value: &SharedString) -> Option<SharedString> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<usize>() {
        Ok(parsed) if parsed >= 1 => None,
        Ok(_) => Some(format!("{label} must be at least 1").into()),
        Err(_) => Some(format!("{label} must be a number").into()),
    }
}

fn validate_roi_value(_label: &str, value: &SharedString) -> Option<SharedString> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<f32>() {
        Ok(parsed) if (0.0..=1.0).contains(&parsed) => None,
        Ok(_) => Some("0-1".into()),
        Err(_) => Some("number".into()),
    }
}

fn parse_optional_u32(label: &str, value: &SharedString) -> Result<Option<u32>, SharedString> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed: u32 = trimmed
        .parse()
        .map_err(|_| format!("{label} must be a number"))?;
    if parsed == 0 {
        return Err(format!("{label} must be at least 1").into());
    }
    Ok(Some(parsed))
}

fn parse_optional_u8(label: &str, value: &SharedString) -> Result<Option<u8>, SharedString> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed: u8 = trimmed
        .parse()
        .map_err(|_| format!("{label} must be 0-255"))?;
    Ok(Some(parsed))
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

    let parse = |label: &str, value: &SharedString, default: f32| -> Result<f32, SharedString> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Ok(default);
        }
        trimmed
            .parse::<f32>()
            .map_err(|_| format!("ROI {label} must be a number").into())
            .and_then(|parsed| {
                if (0.0..=1.0).contains(&parsed) {
                    Ok(parsed)
                } else {
                    Err(format!("ROI {label} must be between 0 and 1").into())
                }
            })
    };

    Ok(Some(RoiFileConfig {
        x: Some(parse("x", x, 0.0)?),
        y: Some(parse("y", y, 0.0)?),
        width: Some(parse("width", width, 1.0)?),
        height: Some(parse("height", height, 1.0)?),
    }))
}

#[derive(Clone, Copy)]
enum ButtonStyle {
    Primary,
    Secondary,
}

#[derive(Clone, Copy)]
enum InputKind {
    Integer,
    Float,
}

struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    input_kind: InputKind,
    blink_start: Instant,
    last_focused: bool,
    invalid: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
}

impl TextInput {
    fn new(
        cx: &mut Context<Self>,
        placeholder: impl Into<SharedString>,
        input_kind: InputKind,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::from(""),
            placeholder: placeholder.into(),
            input_kind,
            blink_start: Instant::now(),
            last_focused: false,
            invalid: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
        }
    }

    fn set_invalid(&mut self, invalid: bool, cx: &mut Context<Self>) {
        if self.invalid != invalid {
            self.invalid = invalid;
            cx.notify();
        }
    }

    fn filter_input(&self, range: &Range<usize>, new_text: &str) -> String {
        match self.input_kind {
            InputKind::Integer => new_text.chars().filter(|ch| ch.is_ascii_digit()).collect(),
            InputKind::Float => {
                let base = format!(
                    "{}{}",
                    &self.content[..range.start],
                    &self.content[range.end..]
                );
                let mut has_dot = base.contains('.');
                let mut filtered = String::new();
                for ch in new_text.chars() {
                    if ch.is_ascii_digit() {
                        filtered.push(ch);
                    } else if ch == '.' && !has_dot {
                        filtered.push(ch);
                        has_dot = true;
                    }
                }
                filtered
            }
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

    fn tab(&mut self, _: &Tab, window: &mut Window, _: &mut Context<Self>) {
        window.focus_next();
    }

    fn tab_prev(&mut self, _: &TabPrev, window: &mut Window, _: &mut Context<Self>) {
        window.focus_prev();
    }

    fn enter(&mut self, _: &Enter, window: &mut Window, _: &mut Context<Self>) {
        window.blur();
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
        let new_text = self.filter_input(&range, new_text);

        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
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
        let new_text = self.filter_input(&range, new_text);

        self.content =
            (self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..])
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

fn snap_to_device(value: Pixels, scale: f32) -> Pixels {
    let snapped = value.scale(scale).round();
    px((f64::from(snapped) as f32) / scale)
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
        let focused = input.focus_handle.is_focused(window);
        let blink_on = if focused {
            (input.blink_start.elapsed().as_millis() / CURSOR_BLINK_INTERVAL.as_millis())
                .is_multiple_of(2)
        } else {
            false
        };
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
        let (selection, cursor) = if selected_range.is_empty() && blink_on {
            let bounds_height: f32 = bounds.size.height.into();
            let font_height: f32 = font_size.into();
            let cursor_height = (font_height * 1.2).min(bounds_height);
            let scale = window.scale_factor();
            let cursor_top = snap_to_device(
                bounds.top() + px((bounds_height - cursor_height) * 0.5),
                scale,
            );
            let cursor_x = snap_to_device(bounds.left() + cursor_pos, scale);
            let cursor_width = px(1.0 / scale);
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(cursor_x, cursor_top),
                        size(cursor_width, px(cursor_height)),
                    ),
                    rgb(0xffffff),
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

        let is_focused = focus_handle.is_focused(window);
        if is_focused && let Some(cursor) = prepaint.cursor.take() {
            window.paint_quad(cursor);
        }
        if is_focused {
            window.request_animation_frame();
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
            if input.last_focused != is_focused {
                input.last_focused = is_focused;
                input.blink_start = Instant::now();
            }
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border_color = if self.invalid {
            hsla(0.0, 0.7, 0.55, 1.0)
        } else {
            rgb(0x2f2f2f).into()
        };
        div()
            .flex()
            .key_context("ConfigTextInput")
            .track_focus(&self.focus_handle(cx))
            .tab_stop(true)
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
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::tab_prev))
            .on_action(cx.listener(Self::enter))
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
                    .flex()
                    .items_center()
                    .h(px(INPUT_HEIGHT))
                    .w_full()
                    .px(px(8.0))
                    .py(px(0.0))
                    .rounded(px(6.0))
                    .bg(rgb(0x101010))
                    .border_1()
                    .border_color(border_color)
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
        KeyBinding::new("tab", Tab, None),
        KeyBinding::new("shift-tab", TabPrev, None),
        KeyBinding::new("enter", Enter, None),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, None),
    ]);
}
