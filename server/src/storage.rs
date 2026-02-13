use crate::api::ApiResponse;
use axum::{extract::Json, response::IntoResponse};
use serde::{Deserialize, Serialize};
use storage::StorageManager;
use tracing::{error, info};

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
            let files = images
                .into_iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect();
            Json(ApiResponse::ok(GetImagesRsp { files })).into_response()
        }
        Err(e) => {
            error!("Failed to get images: {}", e);
            Json(ApiResponse::<GetImagesRsp>::err(
                -1,
                "failed to retrieve images",
            ))
            .into_response()
        }
    }
}

pub async fn mount_image_handler(Json(req): Json<MountImageReq>) -> impl IntoResponse {
    info!(
        "Mount image request: file={}, cdrom={}",
        req.file, req.cdrom
    );

    let file_path = if req.file.is_empty() {
        None
    } else {
        Some(req.file.as_str())
    };

    match StorageManager::mount_image(file_path, req.cdrom).await {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => {
            error!("Failed to mount image: {}", e);
            Json(ApiResponse::<serde_json::Value>::err(
                -1,
                "failed to mount image",
            ))
            .into_response()
        }
    }
}

pub async fn get_mounted_image_handler() -> impl IntoResponse {
    match StorageManager::get_mounted_image().await {
        Ok(image) => Json(ApiResponse::ok(GetMountedImageRsp {
            file: image.unwrap_or_default(),
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to get mounted image: {}", e);
            Json(ApiResponse::<GetMountedImageRsp>::err(
                -1,
                "failed to retrieve mounted image",
            ))
            .into_response()
        }
    }
}

pub async fn get_cdrom_handler() -> impl IntoResponse {
    match StorageManager::get_cdrom_flag().await {
        Ok(flag) => Json(ApiResponse::ok(GetCdRomRsp {
            cdrom: if flag { 1 } else { 0 },
        }))
        .into_response(),
        Err(e) => {
            error!("Failed to get cdrom flag: {}", e);
            Json(ApiResponse::<GetCdRomRsp>::err(
                -1,
                "failed to retrieve cdrom status",
            ))
            .into_response()
        }
    }
}

pub async fn delete_image_handler(Json(req): Json<DeleteImageReq>) -> impl IntoResponse {
    info!("Delete image request: file={}", req.file);

    match StorageManager::delete_image(&req.file).await {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => {
            error!("Failed to delete image: {}", e);
            Json(ApiResponse::<serde_json::Value>::err(
                -1,
                "failed to delete image",
            ))
            .into_response()
        }
    }
}
