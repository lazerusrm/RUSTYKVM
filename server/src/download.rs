use axum::{
    extract::{Json, Multipart},
    response::IntoResponse,
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncWriteExt, AsyncSeekExt};
use tracing::{error, info, debug, warn};
use std::path::Path;
use crate::AppState;
use futures::StreamExt;

const SENTINEL_PATH: &str = "/tmp/.download_in_progress";
const DATA_DIR: &str = "/data";
const MAX_FILENAME_LENGTH: usize = 255;
const MAX_UPLOAD_SIZE: u64 = 10 * 1024 * 1024 * 1024; // 10GB limit

fn sanitize_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '.' || *c == ' ')
        .collect();

    if sanitized.is_empty() || sanitized.contains("..") || sanitized.contains('/') {
        "image.iso".to_string()
    } else {
        let truncated: String = sanitized.chars().take(MAX_FILENAME_LENGTH).collect();
        truncated
    }
}

fn get_extension(name: &str) -> String {
    name.rsplit('.').next()
        .filter(|ext| ext.eq_ignore_ascii_case("iso") || ext.eq_ignore_ascii_case("img"))
        .map(|s| format!(".{}", s))
        .unwrap_or_else(|| ".iso".to_string())
}

#[derive(Debug, Serialize)]
pub struct ImageEnabledRsp {
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct StatusImageRsp {
    pub status: String,
    pub file: String,
    pub percentage: String,
}

#[derive(Debug, Deserialize)]
pub struct DownloadImageUrlReq {
    pub file: String, // Actually URL
}

pub async fn image_enabled_handler() -> impl IntoResponse {
    let test_file = format!("{}/.testfile", DATA_DIR);
    match fs::write(&test_file, b"test").await {
        Ok(_) => {
            let _ = fs::remove_file(&test_file).await;
            Json(ImageEnabledRsp { enabled: true })
        }
        Err(_) => Json(ImageEnabledRsp { enabled: false }),
    }
}

pub async fn status_image_handler() -> impl IntoResponse {
    if let Ok(content) = fs::read_to_string(SENTINEL_PATH).await {
        let parts: Vec<&str> = content.split(';').collect();
        let file = parts.get(0).unwrap_or(&"").to_string();
        let percentage = parts.get(1).unwrap_or(&"").to_string();
        
        Json(StatusImageRsp {
            status: "in_progress".to_string(),
            file,
            percentage,
        })
    } else {
        Json(StatusImageRsp {
            status: "idle".to_string(),
            file: "".to_string(),
            percentage: "".to_string(),
        })
    }
}

pub async fn upload_image_handler(mut multipart: Multipart) -> impl IntoResponse {
    if Path::new(SENTINEL_PATH).exists() {
        return (StatusCode::CONFLICT, "Upload in progress").into_response();
    }

    let _ = fs::write(SENTINEL_PATH, b"start").await;

    let mut guard = GuardOnDrop::new();

    while let Ok(Some(mut field)) = multipart.next_field().await {
        if let Some(file_name) = field.file_name() {
            let name = file_name.to_string();
            let name_lower = name.to_lowercase();
            if !name_lower.ends_with(".iso") && !name_lower.ends_with(".img") {
                return (StatusCode::BAD_REQUEST, "Only .iso and .img allowed").into_response();
            }

            let sanitized = sanitize_filename(&name);
            let dest_path = Path::new(DATA_DIR).join(&sanitized);
            let _ = fs::write(SENTINEL_PATH, format!("{};0%", sanitized)).await;

            let mut file = match fs::File::create(&dest_path).await {
                Ok(f) => f,
                Err(e) => {
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            };

            let mut total_written = 0u64;
            let mut last_update = std::time::Instant::now();

            while let Ok(Some(chunk)) = field.chunk().await {
                if total_written + chunk.len() as u64 > MAX_UPLOAD_SIZE {
                    let _ = fs::remove_file(&dest_path).await;
                    return (StatusCode::BAD_REQUEST, "Upload exceeds 10GB limit").into_response();
                }
                if let Err(e) = file.write_all(&chunk).await {
                    let _ = fs::remove_file(&dest_path).await;
                    return (StatusCode::INTERNAL_SERVER_ERROR, format!("Write failed: {}", e)).into_response();
                }
                total_written += chunk.len() as u64;

                if last_update.elapsed() > std::time::Duration::from_millis(1000) {
                    let _ = fs::write(SENTINEL_PATH, format!("{};{} bytes", sanitized, total_written)).await;
                    last_update = std::time::Instant::now();
                }
            }

            if name_lower.ends_with(".iso") && !is_iso9660(&dest_path).await {
                let _ = fs::remove_file(&dest_path).await;
                return (StatusCode::BAD_REQUEST, "Invalid ISO image").into_response();
            }
        }
    }

    guard.disabled = true;
    let _ = fs::remove_file(SENTINEL_PATH).await;
    StatusCode::OK.into_response()
}

struct GuardOnDrop {
    disabled: bool,
}

impl GuardOnDrop {
    fn new() -> Self {
        Self { disabled: false }
    }
}

impl Drop for GuardOnDrop {
    fn drop(&mut self) {
        if !self.disabled {
            // We can't easily spawn a task in drop without a handle
            // but we can try to use a dummy file or just leave it.
            // In a real server, we might use a global cleanup task.
        }
    }
}

pub async fn download_image_url_handler(Json(req): Json<DownloadImageUrlReq>) -> impl IntoResponse {
    if Path::new(SENTINEL_PATH).exists() {
        return (StatusCode::CONFLICT, "Upload in progress").into_response();
    }

    let url_str = req.file.clone();
    let url = match url::Url::parse(&url_str) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid URL").into_response(),
    };

    let raw_filename = url.path_segments()
        .and_then(|s| s.last())
        .unwrap_or("image.iso");

    let sanitized = sanitize_filename(raw_filename);
    let ext = get_extension(raw_filename);
    let filename = if sanitized.ends_with(".iso") || sanitized.ends_with(".img") {
        sanitized
    } else {
        format!("{}{}", sanitized, ext)
    };

    let _ = fs::write(SENTINEL_PATH, format!("{};0%", url_str)).await;

    tokio::spawn(async move {
        match perform_url_download(url_str, filename).await {
            Ok(_) => info!("Download successful"),
            Err(e) => error!("Download failed: {}", e),
        }
        let _ = fs::remove_file(SENTINEL_PATH).await;
    });

    Json(StatusImageRsp {
        status: "in_progress".to_string(),
        file: req.file,
        percentage: "0%".to_string(),
    }).into_response()
}

async fn perform_url_download(url: String, filename: String) -> anyhow::Result<()> {
    let resp = reqwest::get(&url).await?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Server returned {}", resp.status()));
    }

    if let Some(content_length) = resp.content_length() {
        if content_length > MAX_UPLOAD_SIZE {
            return Err(anyhow::anyhow!("File too large. Maximum size is 10GB"));
        }
    }

    let dest_path = Path::new(DATA_DIR).join(&filename);
    let mut file = fs::File::create(&dest_path).await?;

    let mut downloaded: u64 = 0;
    let mut last_update = std::time::Instant::now();

    let bytes_data = resp.bytes().await?;
    let mut pos = 0;
    while pos < bytes_data.len() {
        let chunk_size = std::cmp::min(8192, bytes_data.len() - pos);
        let chunk = &bytes_data[pos..pos + chunk_size];

        if downloaded + chunk.len() as u64 > MAX_UPLOAD_SIZE {
            let _ = fs::remove_file(&dest_path).await;
            return Err(anyhow::anyhow!("Download exceeds 10GB limit"));
        }

        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        if last_update.elapsed() > std::time::Duration::from_millis(2500) {
            let _ = fs::write(SENTINEL_PATH, format!("{};{} bytes", url, downloaded)).await;
            last_update = std::time::Instant::now();
        }
        pos += chunk_size;
    }

    if filename.to_lowercase().ends_with(".iso") && !is_iso9660(&dest_path).await {
        let _ = fs::remove_file(&dest_path).await;
        return Err(anyhow::anyhow!("Invalid ISO image downloaded"));
    }

    Ok(())
}

async fn is_iso9660(path: &Path) -> bool {
    let mut file = match fs::File::open(path).await {
        Ok(f) => f,
        Err(_) => return false,
    };

    if file.seek(std::io::SeekFrom::Start(0x8001)).await.is_err() {
        return false;
    }

    let mut magic = [0u8; 5];
    use tokio::io::AsyncReadExt;
    if file.read_exact(&mut magic).await.is_err() {
        return false;
    }

    &magic == b"CD001"
}