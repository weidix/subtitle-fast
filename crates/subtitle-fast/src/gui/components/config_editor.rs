use std::env;
use std::fs;
use std::path::PathBuf;

use gpui::Styled;
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Bounds, Context, DispatchPhase, Entity, MouseButton, MouseDownEvent, Pixels,
    PromptButton, PromptLevel, Render, ScrollHandle, SharedString, Subscription, Window,
    WindowBounds, WindowOptions, div, hsla, point, px, rgb, size,
};

use crate::gui::components::Titlebar;
use crate::gui::components::inputs::{InputKind, SelectInput, SelectOption, TextInput};
use crate::gui::menus;
use crate::settings::{
    self, DecoderFileConfig, DetectionFileConfig, FileConfig, OcrFileConfig, OutputFileConfig,
    RoiFileConfig,
};
use subtitle_fast_comparator::Configuration as ComparatorConfiguration;
use subtitle_fast_decoder::Configuration as DecoderConfiguration;
use subtitle_fast_ocr::Configuration as OcrConfiguration;
use subtitle_fast_validator::subtitle_detection::Configuration as DetectorConfiguration;

const FIELD_LABEL_WIDTH: f32 = 200.0;
const ERROR_ROW_HEIGHT: f32 = 14.0;
const ERROR_ROW_PADDING: f32 = 3.0;

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
        let bounds = Bounds::centered(None, size(px(820.0), px(620.0)), cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(820.0), px(620.0))),
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
            let focus_handle = input.read(cx).focus_handle();
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
        selects.into_iter().any(|select| select.read(cx).is_open())
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
                if !select.is_open() {
                    continue;
                }
                let bounds = select.button_bounds()?;
                let options = select.options_snapshot();
                let selected = select.selected_index();
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
                            if !select.is_open() {
                                continue;
                            }
                            if let Some(bounds) = select.button_bounds()
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
            detector_backend: "auto".into(),
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
        let comparator_options = comparator_options();
        let detector_backend_options = detector_backend_options();
        let decoder_backend_options = decoder_backend_options();
        let ocr_backend_options = ocr_backend_options();

        Self {
            sps: cx.new(|cx| TextInput::new(cx, "7", InputKind::Integer)),
            target: cx.new(|cx| TextInput::new(cx, "230", InputKind::Integer)),
            delta: cx.new(|cx| TextInput::new(cx, "12", InputKind::Integer)),
            detector_backend: cx.new(|_| SelectInput::new(detector_backend_options, "auto")),
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
    let mut options = vec![SelectOption::new("auto", "")];
    let available = DecoderConfiguration::available_backends();
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
    let mut options = vec![SelectOption::new("auto", "auto")];
    let mut available = DetectorConfiguration::available_backends();
    let order = ["projection-band", "integral-band", "macos-vision"];
    available.sort_by_key(|backend| {
        order
            .iter()
            .position(|name| *name == backend.as_str())
            .unwrap_or(order.len())
    });
    for backend in available {
        let name = backend.as_str();
        options.push(SelectOption::new(name, name));
    }
    options
}

fn ocr_backend_options() -> Vec<SelectOption> {
    let mut options = vec![SelectOption::new("auto", "auto")];
    let mut available = OcrConfiguration::available_backends();
    let order = ["vision", "ort", "noop"];
    available.sort_by_key(|backend| {
        order
            .iter()
            .position(|name| *name == backend.as_str())
            .unwrap_or(order.len())
    });
    for backend in available {
        let name = backend.as_str();
        options.push(SelectOption::new(name, name));
    }
    options
}

fn comparator_options() -> Vec<SelectOption> {
    let mut options = vec![SelectOption::new("auto", "")];
    let available = ComparatorConfiguration::available_backends();
    for backend in available {
        let name = backend.as_str();
        options.push(SelectOption::new(name, name));
    }
    options
}

fn github_ci_active() -> bool {
    env::var("GITHUB_ACTIONS")
        .map(|value| !value.is_empty() && value != "false")
        .unwrap_or(false)
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
