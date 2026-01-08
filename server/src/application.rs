use axum::{
    extract::{State, Json, Multipart},
    response::IntoResponse,
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{error, info, warn};
use std::path::Path;
use sha2::{Sha512, Digest};
use crate::AppState;
use base64::Engine;

const STABLE_URL: &str = "https://cdn.sipeed.com/nanokvm";
const PREVIEW_URL: &str = "https://cdn.sipeed.com/nanokvm/preview";
const APP_DIR: &str = "/kvmapp";
const BACKUP_DIR: &str = "/root/old";
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
    let current = tokio::fs::read_to_string("/kvmapp/version").await.unwrap_or_else(|_| "1.0.0".to_string());
    let latest = match get_latest_info().await {
        Ok(info) => info.version,
        Err(_) => current.clone(),
    };
    Json(GetVersionRsp { current, latest })
}

pub async fn get_preview_handler() -> impl IntoResponse {
    let enabled = Path::new(PREVIEW_FLAG).exists();
    Json(GetPreviewRsp { enabled })
}

pub async fn set_preview_handler(Json(req): Json<SetPreviewReq>) -> impl IntoResponse {
    if req.enable {
        let _ = tokio::fs::write(PREVIEW_FLAG, b"1").await;
    } else {
        let _ = tokio::fs::remove_file(PREVIEW_FLAG).await;
    }
    StatusCode::OK
}

pub async fn offline_update_handler(mut multipart: Multipart) -> impl IntoResponse {
    if IS_UPDATING.swap(true, Ordering::SeqCst) {
        return (StatusCode::CONFLICT, "Update already in progress").into_response();
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
                    let _ = std::process::Command::new("sh").arg("-c").arg("/etc/init.d/S95nanokvm restart").status();
                });
                return StatusCode::ACCEPTED.into_response();
            }
        }
    }

    IS_UPDATING.store(false, Ordering::SeqCst);
    StatusCode::BAD_REQUEST.into_response()
}

async fn get_latest_info() -> anyhow::Result<LatestInfo> {
    let base_url = if Path::new(PREVIEW_FLAG).exists() { PREVIEW_URL } else { STABLE_URL };
    let url = format!("{}/latest.json?now={}", base_url, chrono::Utc::now().timestamp());
    
    let resp = reqwest::get(url).await?;
    let mut latest: LatestInfo = resp.json().await?;
    latest.url = format!("{}/{}", base_url, latest.name);
    
    Ok(latest)
}

pub async fn update_handler() -> impl IntoResponse {
    if IS_UPDATING.swap(true, Ordering::SeqCst) {
        return (StatusCode::CONFLICT, "Update already in progress").into_response();
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

    StatusCode::ACCEPTED.into_response()
}

async fn perform_update() -> anyhow::Result<()> {
    let _ = tokio::fs::remove_dir_all(CACHE_DIR).await;
    tokio::fs::create_dir_all(CACHE_DIR).await?;

    let latest = get_latest_info().await?;
    let target_path = Path::new(CACHE_DIR).join(&latest.name);

    info!("Downloading update: {} -> {:?}", latest.url, target_path);
    let resp = reqwest::get(&latest.url).await?;
    let bytes = resp.bytes().await?;
    tokio::fs::write(&target_path, &bytes).await?;

    info!("Verifying checksum...");
    let mut hasher = Sha512::new();
    hasher.update(&bytes);
    let hash = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());
    
    if hash != latest.sha512 {
        return Err(anyhow::anyhow!("Checksum mismatch: expected {}, got {}", latest.sha512, hash));
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

    info!("Backing up current installation...");
    let _ = tokio::fs::remove_dir_all(BACKUP_DIR).await;
    tokio::fs::create_dir_all(BACKUP_DIR).await?;
    
    let mut options = fs_extra::dir::CopyOptions::new();
    options.content_only = true;
    fs_extra::dir::move_dir(APP_DIR, BACKUP_DIR, &options)?;

    info!("Applying update...");
    if let Err(e) = fs_extra::dir::move_dir(&extract_dir, APP_DIR, &options) {
        warn!("Update failed, restoring backup: {}", e);
        let _ = fs_extra::dir::move_dir(BACKUP_DIR, APP_DIR, &options);
        return Err(e.into());
    }

    let _ = std::process::Command::new("chmod")
        .arg("-R")
        .arg("755")
        .arg(APP_DIR)
        .status();

    Ok(())
}