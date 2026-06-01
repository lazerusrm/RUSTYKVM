use crate::api::ApiResponse;
use axum::{
    extract::{Json, Multipart},
    response::IntoResponse,
};
use base64::Engine;
use reqwest::header;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use std::env;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{error, info, warn};

// Default update sources:
// - Stable: latest GitHub release asset downloads
// - Preview: optional (only used if enabled via /etc/kvm/preview_updates)
const DEFAULT_STABLE_URL: &str = "https://github.com/lazerusrm/RUSTYKVM/releases/latest/download";
const DEFAULT_PREVIEW_URL: &str = "https://github.com/lazerusrm/RUSTYKVM/releases/download/preview";

const CACHE_DIR: &str = "/root/.kvmcache";
const PREVIEW_FLAG: &str = "/etc/kvm/preview_updates";

static IS_UPDATING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Serialize)]
pub struct GetVersionRsp {
    pub current: String,
    pub latest: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LatestInfo {
    pub version: String,
    pub name: String,
    pub sha512: String,
    pub size: u64,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct GetPreviewRsp {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetPreviewReq {
    pub enable: bool,
}

pub async fn get_version_handler() -> impl IntoResponse {
    let current = tokio::fs::read_to_string("/kvmapp/version")
        .await
        .unwrap_or_else(|_| "1.0.0".to_string());
    let latest = match get_latest_info().await {
        Ok(info) => info.version,
        Err(_) => current.clone(),
    };
    Json(ApiResponse::ok(GetVersionRsp { current, latest }))
}

pub async fn get_preview_handler() -> impl IntoResponse {
    let enabled = Path::new(PREVIEW_FLAG).exists();
    Json(ApiResponse::ok(GetPreviewRsp { enabled }))
}

pub async fn set_preview_handler(Json(req): Json<SetPreviewReq>) -> impl IntoResponse {
    if req.enable {
        let _ = tokio::fs::write(PREVIEW_FLAG, b"1").await;
    } else {
        let _ = tokio::fs::remove_file(PREVIEW_FLAG).await;
    }
    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

fn validate_offline_filename(filename: &str) -> Result<String, String> {
    if filename.contains("..") {
        return Err("invalid filename: path traversal detected".to_string());
    }
    let path = std::path::Path::new(filename);
    let base = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "invalid filename".to_string())?;
    if path != std::path::Path::new(base) {
        return Err("path detected in filename".to_string());
    }
    if !base
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err("invalid filename: contains invalid characters".to_string());
    }
    Ok(base.to_string())
}

fn sha512_base64(data: &[u8]) -> String {
    let mut hasher = Sha512::new();
    hasher.update(data);
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

pub async fn offline_update_handler(mut multipart: Multipart) -> impl IntoResponse {
    use crate::api::error_codes;

    if IS_UPDATING.swap(true, Ordering::SeqCst) {
        return Json(ApiResponse::<serde_json::Value>::err(
            error_codes::GENERIC,
            "update already in progress",
        ))
        .into_response();
    }

    let mut file_bytes: Option<bytes::Bytes> = None;
    let mut upload_name: Option<String> = None;
    let mut expected_sha512: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("file") => {
                upload_name = field.file_name().map(|s| s.to_string());
                file_bytes = field.bytes().await.ok();
            }
            Some("sha512") => {
                if let Ok(text) = field.text().await {
                    let t = text.trim().to_string();
                    if !t.is_empty() {
                        expected_sha512 = Some(t);
                    }
                }
            }
            _ => {}
        }
    }

    let data = match file_bytes {
        Some(d) if !d.is_empty() => d,
        _ => {
            IS_UPDATING.store(false, Ordering::SeqCst);
            return Json(ApiResponse::<serde_json::Value>::err(
                error_codes::VALIDATION,
                "no file uploaded",
            ))
            .into_response();
        }
    };

    let safe_name = match validate_offline_filename(
        upload_name.as_deref().unwrap_or("offline_update.tar.gz"),
    ) {
        Ok(n) => n,
        Err(e) => {
            IS_UPDATING.store(false, Ordering::SeqCst);
            return Json(ApiResponse::<serde_json::Value>::err(error_codes::VALIDATION, &e))
                .into_response();
        }
    };

    let actual_hash = sha512_base64(&data);
    let expected = if let Some(hash) = expected_sha512 {
        hash
    } else {
        match get_latest_info().await {
            Ok(latest) if latest.size == data.len() as u64 => latest.sha512,
            Ok(_) => {
                IS_UPDATING.store(false, Ordering::SeqCst);
                return Json(ApiResponse::<serde_json::Value>::err(
                    error_codes::VALIDATION,
                    "sha512 required (upload size does not match latest.json)",
                ))
                .into_response();
            }
            Err(e) => {
                IS_UPDATING.store(false, Ordering::SeqCst);
                warn!("offline update: cannot resolve expected sha512: {}", e);
                return Json(ApiResponse::<serde_json::Value>::err(
                    error_codes::VALIDATION,
                    "sha512 field required for offline update",
                ))
                .into_response();
            }
        }
    };

    if actual_hash != expected {
        IS_UPDATING.store(false, Ordering::SeqCst);
        return Json(ApiResponse::<serde_json::Value>::err(
            error_codes::VALIDATION,
            "checksum mismatch",
        ))
        .into_response();
    }

    let target = Path::new(CACHE_DIR).join(&safe_name);
    if let Err(e) = tokio::fs::create_dir_all(CACHE_DIR).await {
        IS_UPDATING.store(false, Ordering::SeqCst);
        return Json(ApiResponse::<serde_json::Value>::err(
            error_codes::GENERIC,
            &format!("failed to prepare cache dir: {}", e),
        ))
        .into_response();
    }
    if let Err(e) = tokio::fs::write(&target, &data).await {
        IS_UPDATING.store(false, Ordering::SeqCst);
        return Json(ApiResponse::<serde_json::Value>::err(
            error_codes::GENERIC,
            &format!("failed to write upload: {}", e),
        ))
        .into_response();
    }

    tokio::spawn(async move {
        match install_package(&target).await {
            Ok(()) => info!("Offline update installed from {:?}", target),
            Err(e) => error!("Offline update install failed: {}", e),
        }
        IS_UPDATING.store(false, Ordering::SeqCst);
        let _ = std::process::Command::new("sh")
            .arg("-c")
            .arg("/etc/init.d/S95nanokvm restart")
            .status();
    });

    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

async fn get_latest_info() -> anyhow::Result<LatestInfo> {
    let (stable_url, preview_url) = get_update_base_urls();
    let base_url = if Path::new(PREVIEW_FLAG).exists() {
        preview_url
    } else {
        stable_url
    };

    // Avoid cache-busting query params for GitHub release assets (querystrings are not reliable there).
    // Instead, ask intermediaries not to cache.
    let url = format!("{}/latest.json", base_url);
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::PRAGMA, "no-cache")
        .send()
        .await?;
    let mut latest: LatestInfo = resp.json().await?;
    if latest.url.is_empty() {
        latest.url = format!("{}/{}", base_url, latest.name);
    }

    Ok(latest)
}

pub async fn update_handler() -> impl IntoResponse {
    if IS_UPDATING.swap(true, Ordering::SeqCst) {
        return Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "update already in progress",
        ))
        .into_response();
    }

    tokio::spawn(async move {
        match perform_update().await {
            Ok(_) => {
                info!("Update successful, restarting...");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg("/etc/init.d/S95nanokvm restart")
                    .status();
            }
            Err(e) => {
                error!("Update failed: {}", e);
            }
        }
        IS_UPDATING.store(false, Ordering::SeqCst);
    });

    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

async fn perform_update() -> anyhow::Result<()> {
    let _ = tokio::fs::remove_dir_all(CACHE_DIR).await;
    tokio::fs::create_dir_all(CACHE_DIR).await?;

    let latest = get_latest_info().await?;
    let target_path = Path::new(CACHE_DIR).join(&latest.name);

    info!("Downloading update: {} -> {:?}", latest.url, target_path);
    let client = reqwest::Client::new();
    let resp = client
        .get(&latest.url)
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::PRAGMA, "no-cache")
        .send()
        .await?;
    let bytes = resp.bytes().await?;
    tokio::fs::write(&target_path, &bytes).await?;

    info!("Verifying checksum...");
    let mut hasher = Sha512::new();
    hasher.update(&bytes);
    let hash = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());

    if hash != latest.sha512 {
        return Err(anyhow::anyhow!(
            "Checksum mismatch: expected {}, got {}",
            latest.sha512,
            hash
        ));
    }

    info!("Installing package...");
    install_package(&target_path).await?;

    Ok(())
}

async fn install_package(archive_path: &Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(archive_path)?;
    let tar = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(tar);

    let extract_dir = Path::new(CACHE_DIR).join("extracted");
    let _ = tokio::fs::remove_dir_all(&extract_dir).await;
    tokio::fs::create_dir_all(&extract_dir).await?;
    archive.unpack(&extract_dir)?;

    // Run installer from within the package.
    // This avoids replacing the entire /kvmapp tree, and matches how our GitHub upgrade tarballs are structured.
    let install_sh = extract_dir.join("install.sh");
    if !install_sh.exists() {
        return Err(anyhow::anyhow!(
            "update package missing install.sh at {:?}",
            install_sh
        ));
    }

    info!("Applying update via install.sh...");
    let status = std::process::Command::new("sh")
        .arg("-c")
        // The updater runs from within the server process; do not stop/start the service inside
        // the install script or it can kill the running updater mid-install.
        .arg("chmod +x ./install.sh; NANOKVM_SKIP_SERVICE=1 ./install.sh")
        .current_dir(&extract_dir)
        .status()?;
    if !status.success() {
        warn!("install.sh failed with status: {}", status);
        return Err(anyhow::anyhow!("install.sh failed"));
    }

    Ok(())
}

fn get_update_base_urls() -> (String, String) {
    let stable =
        env::var("NANOKVM_UPDATE_STABLE_URL").unwrap_or_else(|_| DEFAULT_STABLE_URL.into());
    let preview =
        env::var("NANOKVM_UPDATE_PREVIEW_URL").unwrap_or_else(|_| DEFAULT_PREVIEW_URL.into());
    (
        stable.trim_end_matches('/').to_string(),
        preview.trim_end_matches('/').to_string(),
    )
}
