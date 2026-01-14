//! Model asset management for the ORT OCR backend.

use std::env;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

#[cfg(feature = "ocr-ort")]
use futures_util::StreamExt;
#[cfg(feature = "ocr-ort")]
use tokio::fs;
#[cfg(feature = "ocr-ort")]
use tokio::io::AsyncWriteExt;

use crate::settings;

const ORT_MODEL_FILENAME: &str = "ch_PP-OCRv5_rec_infer.onnx";
const ORT_DICT_FILENAME: &str = "ch_PP-OCRv5_rec_infer.txt";
#[cfg(feature = "ocr-ort")]
const ORT_MODEL_URL: &str = "https://raw.githubusercontent.com/weidix/subtitle-fast/master/models/ch_PP-OCRv5_rec_infer.onnx";
#[cfg(feature = "ocr-ort")]
const ORT_DICT_URL: &str = "https://raw.githubusercontent.com/weidix/subtitle-fast/master/models/ch_PP-OCRv5_rec_infer.txt";

static ORT_MODEL_PATHS: OnceLock<OrtModelPaths> = OnceLock::new();

/// Resolved file paths for ORT OCR assets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrtModelPaths {
    model_path: PathBuf,
    dictionary_path: PathBuf,
}

impl OrtModelPaths {
    /// Path to the ONNX model file.
    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    /// Path to the dictionary text file.
    pub fn dictionary_path(&self) -> &Path {
        &self.dictionary_path
    }
}

/// Errors raised when resolving ORT model paths.
#[derive(Debug)]
pub enum ModelPathError {
    MissingConfigDir,
    InvalidConfigPath {
        path: PathBuf,
    },
    ConflictingModelPaths {
        existing: PathBuf,
        requested: PathBuf,
    },
}

impl fmt::Display for ModelPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingConfigDir => write!(f, "unable to resolve a config directory"),
            Self::InvalidConfigPath { path } => {
                write!(
                    f,
                    "config path '{}' has no parent directory",
                    path.display()
                )
            }
            Self::ConflictingModelPaths {
                existing,
                requested,
            } => write!(
                f,
                "model paths already set to '{}', requested '{}'",
                existing.display(),
                requested.display()
            ),
        }
    }
}

impl std::error::Error for ModelPathError {}

/// Download progress events emitted while fetching ORT assets.
#[derive(Debug, Clone)]
pub enum ModelDownloadEvent {
    Started {
        file_label: String,
        file_index: usize,
        file_count: usize,
        total_bytes: Option<u64>,
    },
    Progress {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
    Finished {
        file_label: String,
    },
    Completed,
    Failed {
        message: String,
    },
}

/// Errors raised while downloading ORT model assets.
#[derive(Debug)]
pub enum ModelDownloadError {
    Unsupported {
        message: String,
    },
    RequestFailed {
        url: String,
        message: String,
    },
    HttpStatus {
        url: String,
        status: u16,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl fmt::Display for ModelDownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { message } => write!(f, "{message}"),
            Self::RequestFailed { url, message } => {
                write!(f, "request to '{url}' failed: {message}")
            }
            Self::HttpStatus { url, status } => {
                write!(f, "request to '{url}' returned status {status}")
            }
            Self::Io { path, source } => write!(
                f,
                "failed to write model asset at '{}': {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ModelDownloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Resolve and cache ORT model paths using a config file location.
pub fn init_ort_model_paths(config_path: Option<&Path>) -> Result<OrtModelPaths, ModelPathError> {
    let resolved = resolve_ort_model_paths(config_path)?;
    match ORT_MODEL_PATHS.set(resolved.clone()) {
        Ok(()) => Ok(resolved),
        Err(existing) => {
            if existing == resolved {
                Ok(existing)
            } else {
                Err(ModelPathError::ConflictingModelPaths {
                    existing: existing.model_path.clone(),
                    requested: resolved.model_path,
                })
            }
        }
    }
}

/// Get the cached ORT model paths, falling back to the default config directory.
pub fn ort_model_paths() -> Result<OrtModelPaths, ModelPathError> {
    if let Some(paths) = ORT_MODEL_PATHS.get() {
        return Ok(paths.clone());
    }

    let resolved = resolve_ort_model_paths(None)?;
    let _ = ORT_MODEL_PATHS.set(resolved.clone());
    Ok(resolved)
}

/// Returns true when both ORT assets are present and non-empty.
pub fn ort_models_present(paths: &OrtModelPaths) -> bool {
    file_ready(&paths.model_path) && file_ready(&paths.dictionary_path)
}

#[cfg(feature = "ocr-ort")]
/// Download missing ORT model assets, emitting progress events as data arrives.
pub async fn download_ort_models(
    paths: &OrtModelPaths,
    on_event: Option<Arc<dyn Fn(ModelDownloadEvent) + Send + Sync>>,
) -> Result<(), ModelDownloadError> {
    let assets = missing_assets(paths);
    if assets.is_empty() {
        return Ok(());
    }

    let model_dir = paths
        .model_path
        .parent()
        .ok_or_else(|| ModelDownloadError::Io {
            path: paths.model_path.clone(),
            source: std::io::Error::other("model path has no parent directory"),
        })?;
    fs::create_dir_all(model_dir)
        .await
        .map_err(|err| ModelDownloadError::Io {
            path: model_dir.to_path_buf(),
            source: err,
        })?;

    let client = reqwest::Client::new();
    let total_files = assets.len();
    for (index, asset) in assets.iter().enumerate() {
        download_asset(&client, asset, index + 1, total_files, on_event.as_ref()).await?;
    }

    Ok(())
}

#[cfg(not(feature = "ocr-ort"))]
/// Downloading ORT model assets requires the `ocr-ort` feature.
pub async fn download_ort_models(
    _paths: &OrtModelPaths,
    _on_event: Option<Arc<dyn Fn(ModelDownloadEvent) + Send + Sync>>,
) -> Result<(), ModelDownloadError> {
    Err(ModelDownloadError::Unsupported {
        message: "ort backend is disabled in this build".to_string(),
    })
}

fn resolve_ort_model_paths(config_path: Option<&Path>) -> Result<OrtModelPaths, ModelPathError> {
    let config_dir = resolve_config_dir(config_path)?;
    let model_dir = config_dir.join("models");
    Ok(OrtModelPaths {
        model_path: model_dir.join(ORT_MODEL_FILENAME),
        dictionary_path: model_dir.join(ORT_DICT_FILENAME),
    })
}

fn resolve_config_dir(config_path: Option<&Path>) -> Result<PathBuf, ModelPathError> {
    if let Some(path) = config_path {
        let parent = path
            .parent()
            .ok_or_else(|| ModelPathError::InvalidConfigPath {
                path: path.to_path_buf(),
            })?;
        return Ok(parent.to_path_buf());
    }

    if let Ok(cwd) = env::current_dir() {
        let project_config = cwd.join("config.toml");
        if project_config.exists() {
            return Ok(cwd);
        }
    }

    let default_config = settings::default_config_path().ok_or(ModelPathError::MissingConfigDir)?;
    let parent = default_config
        .parent()
        .ok_or_else(|| ModelPathError::InvalidConfigPath {
            path: default_config.clone(),
        })?;
    Ok(parent.to_path_buf())
}

#[cfg(feature = "ocr-ort")]
fn missing_assets(paths: &OrtModelPaths) -> Vec<ModelAsset> {
    let mut assets = Vec::new();
    if !file_ready(&paths.model_path) {
        assets.push(ModelAsset::new(
            "OCR model",
            ORT_MODEL_URL,
            paths.model_path.clone(),
        ));
    }
    if !file_ready(&paths.dictionary_path) {
        assets.push(ModelAsset::new(
            "OCR dictionary",
            ORT_DICT_URL,
            paths.dictionary_path.clone(),
        ));
    }
    assets
}

fn file_ready(path: &Path) -> bool {
    path.metadata().map(|meta| meta.len() > 0).unwrap_or(false)
}

#[cfg(feature = "ocr-ort")]
#[derive(Debug, Clone)]
struct ModelAsset {
    label: &'static str,
    url: &'static str,
    path: PathBuf,
}

#[cfg(feature = "ocr-ort")]
impl ModelAsset {
    fn new(label: &'static str, url: &'static str, path: PathBuf) -> Self {
        Self { label, url, path }
    }
}

#[cfg(feature = "ocr-ort")]
async fn download_asset(
    client: &reqwest::Client,
    asset: &ModelAsset,
    file_index: usize,
    file_count: usize,
    on_event: Option<&Arc<dyn Fn(ModelDownloadEvent) + Send + Sync>>,
) -> Result<(), ModelDownloadError> {
    let response =
        client
            .get(asset.url)
            .send()
            .await
            .map_err(|err| ModelDownloadError::RequestFailed {
                url: asset.url.to_string(),
                message: err.to_string(),
            })?;

    if !response.status().is_success() {
        return Err(ModelDownloadError::HttpStatus {
            url: asset.url.to_string(),
            status: response.status().as_u16(),
        });
    }

    let total_bytes = response.content_length();
    emit_event(
        on_event,
        ModelDownloadEvent::Started {
            file_label: asset.label.to_string(),
            file_index,
            file_count,
            total_bytes,
        },
    );

    let tmp_path = asset.path.with_extension("part");
    if let Some(parent) = tmp_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|err| ModelDownloadError::Io {
                path: parent.to_path_buf(),
                source: err,
            })?;
    }

    if asset.path.exists() {
        fs::remove_file(&asset.path)
            .await
            .map_err(|err| ModelDownloadError::Io {
                path: asset.path.clone(),
                source: err,
            })?;
    }

    let mut file = fs::File::create(&tmp_path)
        .await
        .map_err(|err| ModelDownloadError::Io {
            path: tmp_path.clone(),
            source: err,
        })?;

    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| ModelDownloadError::RequestFailed {
            url: asset.url.to_string(),
            message: err.to_string(),
        })?;

        if chunk.is_empty() {
            continue;
        }

        file.write_all(&chunk)
            .await
            .map_err(|err| ModelDownloadError::Io {
                path: tmp_path.clone(),
                source: err,
            })?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);

        emit_event(
            on_event,
            ModelDownloadEvent::Progress {
                downloaded_bytes: downloaded,
                total_bytes,
            },
        );
    }

    file.flush().await.map_err(|err| ModelDownloadError::Io {
        path: tmp_path.clone(),
        source: err,
    })?;

    fs::rename(&tmp_path, &asset.path)
        .await
        .map_err(|err| ModelDownloadError::Io {
            path: asset.path.clone(),
            source: err,
        })?;

    emit_event(
        on_event,
        ModelDownloadEvent::Finished {
            file_label: asset.label.to_string(),
        },
    );

    Ok(())
}

#[cfg(feature = "ocr-ort")]
fn emit_event(
    on_event: Option<&Arc<dyn Fn(ModelDownloadEvent) + Send + Sync>>,
    event: ModelDownloadEvent,
) {
    if let Some(handler) = on_event {
        handler(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolve_ort_paths_use_models_subdir() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        let paths = resolve_ort_model_paths(Some(&config_path)).unwrap();
        assert_eq!(
            paths.model_path,
            dir.path().join("models").join(ORT_MODEL_FILENAME)
        );
        assert_eq!(
            paths.dictionary_path,
            dir.path().join("models").join(ORT_DICT_FILENAME)
        );
    }

    #[test]
    fn ort_models_present_requires_non_empty_files() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.onnx");
        let dict_path = dir.path().join("dict.txt");
        let paths = OrtModelPaths {
            model_path: model_path.clone(),
            dictionary_path: dict_path.clone(),
        };

        assert!(!ort_models_present(&paths));
        std::fs::write(&model_path, []).unwrap();
        std::fs::write(&dict_path, [1u8]).unwrap();
        assert!(!ort_models_present(&paths));
        std::fs::write(&model_path, [1u8]).unwrap();
        assert!(ort_models_present(&paths));
    }
}
