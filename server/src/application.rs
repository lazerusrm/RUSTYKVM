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

pub async fn offline_update_handler(mut multipart: Multipart) -> impl IntoResponse {
    if IS_UPDATING.swap(true, Ordering::SeqCst) {
        return Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "update already in progress",
        ))
        .into_response();
    }

    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            if let Ok(data) = field.bytes().await {
                let target = Path::new(CACHE_DIR).join("offline_update.tar.gz");
                let _ = tokio::fs::create_dir_all(CACHE_DIR).await;
                let _ = tokio::fs::write(&target, data).await;

                tokio::spawn(async move {
                    let _ = install_package(&target).await;
                    IS_UPDATING.store(false, Ordering::SeqCst);
                    let _ = std::process::Command::new("sh")
                        .arg("-c")
                        .arg("/etc/init.d/S95nanokvm restart")
                        .status();
                });
                // Go-style API always returns HTTP 200 with `{code,msg,data}`.
                // Frontend offline updater incorrectly checks `!rsp.data`, so return `{}`.
                return Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response();
            }
        }
    }

    IS_UPDATING.store(false, Ordering::SeqCst);
    Json(ApiResponse::<serde_json::Value>::err(
        -1,
        "no file uploaded",
    ))
    .into_response()
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
