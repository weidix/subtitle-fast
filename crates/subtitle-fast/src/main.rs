#![cfg_attr(
    all(target_os = "windows", feature = "gui"),
    windows_subsystem = "windows"
)]

use std::env;
use std::io::{self, IsTerminal, Write};
use std::num::NonZeroUsize;
use std::sync::Arc;

use clap::CommandFactory;
use subtitle_fast::backend::{self, ExecutionPlan};
use subtitle_fast::cli::{CliArgs, CliSources, parse_cli};
#[cfg(feature = "gui")]
use subtitle_fast::gui::SubtitleFastApp;
use subtitle_fast::model;
use subtitle_fast::settings::{ConfigError, resolve_settings};
use subtitle_fast::stage::PipelineConfig;
use subtitle_fast_types::DecoderError;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), DecoderError> {
    #[allow(unused_variables)]
    let args: Vec<String> = env::args().collect();

    #[cfg(feature = "gui")]
    {
        if args.len() == 1 {
            return run_gui();
        }
    }

    run_cli().await
}

#[cfg(feature = "gui")]
fn run_gui() -> Result<(), DecoderError> {
    use futures_channel::mpsc::unbounded;
    use gpui::*;
    use subtitle_fast::gui::components::DownloadWindow;
    use subtitle_fast::gui::components::bind_text_input_keys;
    use subtitle_fast::gui::{AppAssets, menus, runtime};

    let app = Application::new().with_assets(AppAssets);
    #[cfg(target_os = "macos")]
    {
        app.on_reopen(|cx| {
            if cx.windows().is_empty() {
                open_main_window(cx);
            } else {
                cx.activate(true);
            }
        });
    }
    app.run(|cx: &mut App| {
        runtime::init(tokio::runtime::Handle::current());
        bind_text_input_keys(cx);
        menus::register_actions(cx);
        #[cfg(target_os = "macos")]
        cx.bind_keys([gpui::KeyBinding::new(
            "cmd-q",
            subtitle_fast::gui::menus::Quit,
            None,
        )]);
        if cfg!(target_os = "macos") {
            menus::set_macos_menus(cx, &[], false);
        } else {
            menus::set_app_menus(cx, &[], false);
        }
        let settings = subtitle_fast::settings::resolve_gui_settings().ok();
        let model_paths = match model::init_ort_model_paths(None) {
            Ok(paths) => Some(paths),
            Err(err) => {
                eprintln!("ort model path resolution failed: {err}");
                None
            }
        };

        if should_prepare_ort(settings.as_ref()) {
            match model_paths {
                Some(paths) if !model::ort_models_present(&paths) => {
                    let Some(handle) = DownloadWindow::open(cx) else {
                        open_main_window(cx);
                        return;
                    };
                    let (progress_tx, progress_rx) = unbounded::<model::ModelDownloadEvent>();
                    let on_continue = Arc::new(move |window: &mut Window, cx: &mut App| {
                        window.remove_window();
                        open_main_window(cx);
                    });
                    let on_exit = Arc::new(move |window: &mut Window, cx: &mut App| {
                        window.remove_window();
                        cx.quit();
                    });
                    let _ = handle.update(cx, |this, window, cx| {
                        this.bind_progress(
                            progress_rx,
                            handle,
                            on_continue.clone(),
                            on_exit.clone(),
                            window,
                            cx,
                        );
                    });

                    let progress_tx_events = progress_tx.clone();
                    let progress_tx_result = progress_tx.clone();
                    let progress_callback = Arc::new(move |event| {
                        let _ = progress_tx_events.unbounded_send(event);
                    });
                    let download_paths = paths.clone();

                    if runtime::spawn(async move {
                        let result =
                            model::download_ort_models(&download_paths, Some(progress_callback))
                                .await;
                        let final_event = match result {
                            Ok(()) => model::ModelDownloadEvent::Completed,
                            Err(err) => model::ModelDownloadEvent::Failed {
                                message: err.to_string(),
                            },
                        };
                        let _ = progress_tx_result.unbounded_send(final_event);
                    })
                    .is_none()
                    {
                        let _ = progress_tx.unbounded_send(model::ModelDownloadEvent::Failed {
                            message: "tokio runtime not initialized".to_string(),
                        });
                    }
                }
                Some(_) => open_main_window(cx),
                None => {
                    let Some(handle) = DownloadWindow::open(cx) else {
                        open_main_window(cx);
                        return;
                    };
                    let (progress_tx, progress_rx) = unbounded::<model::ModelDownloadEvent>();
                    let on_continue = Arc::new(move |window: &mut Window, cx: &mut App| {
                        window.remove_window();
                        open_main_window(cx);
                    });
                    let on_exit = Arc::new(move |window: &mut Window, cx: &mut App| {
                        window.remove_window();
                        cx.quit();
                    });
                    let _ = handle.update(cx, |this, window, cx| {
                        this.bind_progress(
                            progress_rx,
                            handle,
                            on_continue.clone(),
                            on_exit.clone(),
                            window,
                            cx,
                        );
                    });
                    let _ = progress_tx.unbounded_send(model::ModelDownloadEvent::Failed {
                        message: "unable to resolve model paths".to_string(),
                    });
                }
            }
        } else {
            open_main_window(cx);
        }
    });

    Ok(())
}

async fn run_cli() -> Result<(), DecoderError> {
    match prepare_execution_plan().await? {
        Some(plan) => backend::run(plan).await,
        None => Ok(()),
    }
}

async fn prepare_execution_plan() -> Result<Option<ExecutionPlan>, DecoderError> {
    let (cli_args, cli_sources): (CliArgs, CliSources) = parse_cli();

    if cli_args.list_backends {
        backend::display_available_backends();
        return Ok(None);
    }

    let input = match cli_args.input.clone() {
        Some(path) => path,
        None => {
            usage();
            return Ok(None);
        }
    };

    if !input.exists() {
        return Err(DecoderError::configuration(format!(
            "input file '{}' does not exist",
            input.display()
        )));
    }

    let resolved = resolve_settings(&cli_args, &cli_sources).map_err(map_config_error)?;
    let settings = resolved.settings;
    let model_paths = model::init_ort_model_paths(resolved.config_path.as_deref())
        .map_err(|err| DecoderError::configuration(err.to_string()))?;

    if should_prepare_ort(Some(&settings)) && !model::ort_models_present(&model_paths) {
        let proceed = ensure_ort_models_cli(&model_paths).await?;
        if !proceed {
            return Ok(None);
        }
    }

    let pipeline = PipelineConfig::from_settings(&settings, &input)?;

    let env_backend_present = std::env::var("SUBFAST_BACKEND").is_ok();
    let mut config = subtitle_fast_decoder::Configuration::from_env().unwrap_or_default();
    let backend_override = match settings.decoder.backend.as_ref() {
        Some(name) => Some(backend::parse_backend(name)?),
        None => None,
    };
    let backend_locked = backend_override.is_some() || env_backend_present;
    if let Some(backend_value) = backend_override {
        config.backend = backend_value;
    }
    config.input = Some(input);
    if let Some(capacity) = settings.decoder.channel_capacity
        && let Some(non_zero) = NonZeroUsize::new(capacity)
    {
        config.channel_capacity = Some(non_zero);
    }

    Ok(Some(ExecutionPlan {
        config,
        backend_locked,
        pipeline,
    }))
}

fn usage() {
    let mut command = CliArgs::command();
    command.print_help().ok();
    println!();
    backend::display_available_backends();
}

fn map_config_error(err: ConfigError) -> DecoderError {
    DecoderError::configuration(err.to_string())
}

#[cfg(feature = "gui")]
fn open_main_window(cx: &mut gpui::App) {
    let app = SubtitleFastApp::new(cx);
    app.open_window(cx);
    cx.activate(true);
}

fn should_prepare_ort(settings: Option<&subtitle_fast::settings::EffectiveSettings>) -> bool {
    if !cfg!(feature = "ocr-ort") {
        return false;
    }

    let backend = settings
        .and_then(|settings| settings.ocr.backend.as_ref())
        .map(|value| value.trim().to_ascii_lowercase());
    let selected = backend.as_deref().unwrap_or("auto");
    if selected == "ort" {
        return true;
    }
    if selected == "vision" || selected == "noop" {
        return false;
    }
    !cfg!(all(feature = "ocr-vision", target_os = "macos"))
}

async fn ensure_ort_models_cli(paths: &model::OrtModelPaths) -> Result<bool, DecoderError> {
    let progress = indicatif::ProgressBar::new(0);
    let progress_for_events = progress.clone();
    progress.set_style(download_spinner_style());

    let progress_handler = Arc::new(move |event: model::ModelDownloadEvent| match event {
        model::ModelDownloadEvent::Started {
            file_label,
            file_index,
            file_count,
            total_bytes,
        } => {
            progress_for_events.set_message(format!(
                "downloading {file_label} ({file_index}/{file_count})"
            ));
            if let Some(total) = total_bytes {
                progress_for_events.set_style(download_bar_style());
                progress_for_events.set_length(total);
                progress_for_events.set_position(0);
            } else {
                progress_for_events.set_style(download_spinner_style());
                progress_for_events.enable_steady_tick(std::time::Duration::from_millis(120));
            }
        }
        model::ModelDownloadEvent::Progress {
            downloaded_bytes,
            total_bytes,
        } => {
            if let Some(total) = total_bytes {
                progress_for_events.set_length(total);
                progress_for_events.set_position(downloaded_bytes);
            } else {
                progress_for_events.tick();
            }
        }
        model::ModelDownloadEvent::Finished { file_label } => {
            progress_for_events.set_message(format!("downloaded {file_label}"));
        }
        _ => {}
    });

    let result = model::download_ort_models(paths, Some(progress_handler)).await;
    match result {
        Ok(()) => {
            progress.finish_with_message("model download complete");
            Ok(true)
        }
        Err(err) => {
            progress.finish_with_message("model download failed");
            prompt_continue_after_download_failure(&err)
        }
    }
}

fn prompt_continue_after_download_failure(
    err: &model::ModelDownloadError,
) -> Result<bool, DecoderError> {
    eprintln!("ort model download failed: {err}");
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("continuing without the ORT model in non-interactive mode");
        return Ok(true);
    }

    eprint!("continue without the ORT model? [y/N]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let reply = input.trim().to_ascii_lowercase();
    Ok(reply == "y" || reply == "yes")
}

fn download_bar_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::with_template(
        "[{elapsed_precise}] {bar:40.cyan/blue} {bytes}/{total_bytes} {msg}",
    )
    .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
    .progress_chars("##-")
}

fn download_spinner_style() -> indicatif::ProgressStyle {
    indicatif::ProgressStyle::with_template("[{elapsed_precise}] {spinner} {msg}")
        .unwrap_or_else(|_| indicatif::ProgressStyle::default_spinner())
}
