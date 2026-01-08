use axum::{
    extract::Json,
    response::IntoResponse,
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::{error, info};
use storage::StorageManager;

#[derive(Serialize)]
pub struct GetImagesRsp {
    pub files: Vec<String>,
}

#[derive(Deserialize)]
pub struct MountImageReq {
    pub file: String,
    pub cdrom: bool,
}

#[derive(Serialize)]
pub struct GetMountedImageRsp {
    pub file: String,
}

#[derive(Serialize)]
pub struct GetCdRomRsp {
    pub cdrom: i64,
}

#[derive(Deserialize)]
pub struct DeleteImageReq {
    pub file: String,
}

pub async fn get_images_handler() -> impl IntoResponse {
    match StorageManager::get_images() {
        Ok(images) => {
            let files = images.into_iter().map(|p| p.to_string_lossy().to_string()).collect();
            Json(GetImagesRsp { files }).into_response()
        }
        Err(e) => {
            error!("Failed to get images: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

pub async fn mount_image_handler(Json(req): Json<MountImageReq>) -> impl IntoResponse {
    info!("Mount image request: file={}, cdrom={}", req.file, req.cdrom);
    
    let file_path = if req.file.is_empty() { None } else { Some(req.file.as_str()) };
    
    match StorageManager::mount_image(file_path, req.cdrom).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            error!("Failed to mount image: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

pub async fn get_mounted_image_handler() -> impl IntoResponse {
    match StorageManager::get_mounted_image().await {
        Ok(image) => {
            Json(GetMountedImageRsp { file: image.unwrap_or_default() }).into_response()
        }
        Err(e) => {
            error!("Failed to get mounted image: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

pub async fn get_cdrom_handler() -> impl IntoResponse {
    match StorageManager::get_cdrom_flag().await {
        Ok(flag) => {
            Json(GetCdRomRsp { cdrom: if flag { 1 } else { 0 } }).into_response()
        }
        Err(e) => {
            error!("Failed to get cdrom flag: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

pub async fn delete_image_handler(Json(req): Json<DeleteImageReq>) -> impl IntoResponse {
    info!("Delete image request: file={}", req.file);
    
    match StorageManager::delete_image(&req.file).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            error!("Failed to delete image: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}
