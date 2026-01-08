use axum::{
    extract::{State, Json, Multipart},
    response::IntoResponse,
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncSeekExt};
use tracing::{error, info, debug, warn};
use std::path::{Path, PathBuf};
use crate::AppState;

const SENTINEL_PATH: &str = "/tmp/.download_in_progress";
const DATA_DIR: &str = "/data";

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
        return (StatusCode::CONFLICT, "Download in progress").into_response();
    }

    let _ = fs::write(SENTINEL_PATH, b"start").await;

    while let Ok(Some(field)) = multipart.next_field().await {
        if let Some(file_name) = field.file_name() {
            let name = file_name.to_string();
            if !name.to_lowercase().ends_with(".iso") {
                let _ = fs::remove_file(SENTINEL_PATH).await;
                return (StatusCode::BAD_REQUEST, "Only .iso allowed").into_response();
            }

            let dest_path = Path::new(DATA_DIR).join(&name);
            let _ = fs::write(SENTINEL_PATH, format!("{};0%", name)).await;

            let mut file = match fs::File::create(&dest_path).await {
                Ok(f) => f,
                Err(e) => {
                    let _ = fs::remove_file(SENTINEL_PATH).await;
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            };

            // Simplified streaming without complex ticker for now
            // In a real implementation we'd update percentage periodically
            let mut reader = field;
            let mut total_written = 0;
            let mut buffer = [0u8; 8192];
            let mut last_update = std::time::Instant::now();
            
            // Try to get content length for percentage
            // Axum multipart might not give it easily per field if not provided by client
            
            while let Ok(n) = reader.read(&mut buffer).await {
                if n == 0 { break; }
                if file.write_all(&buffer[..n]).await.is_err() {
                    let _ = fs::remove_file(SENTINEL_PATH).await;
                    let _ = fs::remove_file(&dest_path).await;
                    return (StatusCode::INTERNAL_SERVER_ERROR, "Write failed").into_response();
                }
                total_written += n;

                if last_update.elapsed() > std::time::Duration::from_millis(1000) {
                    // Update sentinel with current written size if we don't have total
                    let _ = fs::write(SENTINEL_PATH, format!("{};{} bytes", name, total_written)).await;
                    last_update = std::time::Instant::now();
                }
            }

            if !is_iso9660(&dest_path).await {
                let _ = fs::remove_file(SENTINEL_PATH).await;
                let _ = fs::remove_file(&dest_path).await;
                return (StatusCode::BAD_REQUEST, "Invalid ISO image").into_response();
            }
        }
    }

    let _ = fs::remove_file(SENTINEL_PATH).await;
    StatusCode::OK.into_response()
}

pub async fn download_image_url_handler(Json(req): Json<DownloadImageUrlReq>) -> impl IntoResponse {
    if Path::new(SENTINEL_PATH).exists() {
        return (StatusCode::CONFLICT, "Download in progress").into_response();
    }

    let url = match url::Url::parse(&req.file) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid URL").into_response(),
    };

    let filename = url.path_segments()
        .and_then(|s| s.last())
        .unwrap_or("image.iso")
        .to_string();

    let _ = fs::write(SENTINEL_PATH, format!("{};0%", req.file)).await;

    tokio::spawn(async move {
        match perform_url_download(req.file, filename).await {
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

    let total_size = resp.content_length().unwrap_or(0);
    let mut stream = resp.bytes_stream();
    let dest_path = Path::new(DATA_DIR).join(&filename);
    let mut file = fs::File::create(&dest_path).await?;

    let mut downloaded: u64 = 0;
    let mut last_update = std::time::Instant::now();

    while let Some(item) = stream.next().await {
        let chunk = item?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;

        if last_update.elapsed() > std::time::Duration::from_millis(2500) && total_size > 0 {
            let percentage = (downloaded as f64 / total_size as f64) * 100.0;
            let _ = fs::write(SENTINEL_PATH, format!("{};{:.2}%", url, percentage)).await;
            last_update = std::time::Instant::now();
        }
    }

    if !is_iso9660(&dest_path).await {
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
    if file.read_exact(&mut magic).await.is_err() {
        return false;
    }

    &magic == b"CD001"
}
