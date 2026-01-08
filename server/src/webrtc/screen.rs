use parking_lot::RwLock;
use serde::Deserialize;
use std::sync::Arc;

#[cfg(target_os = "linux")]
use crate::AppState;
#[cfg(target_os = "linux")]
use axum::http::StatusCode;
#[cfg(target_os = "linux")]
use axum::{
    extract::{Json, State},
    response::IntoResponse,
};

pub struct ScreenConfig {
    pub stream_type: String, // "mjpeg" or "h264"
    pub fps: u16,
    pub quality: u16,
    pub width: u16,
    pub height: u16,
    pub bitrate: u16,
    pub gop: u8,
}

impl ScreenConfig {
    pub fn new() -> Self {
        Self {
            stream_type: "mjpeg".to_string(),
            fps: 30,
            quality: 80,
            width: 1280,
            height: 720,
            bitrate: 2000,
            gop: 30,
        }
    }
}

pub type SharedScreenConfig = Arc<RwLock<ScreenConfig>>;

#[derive(Debug, Deserialize)]
pub struct UpdateFrameDetectReq {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct StopFrameDetectReq {
    pub duration: Option<u64>,
}

const FRAME_DETECT_INTERVAL: u8 = 60;

#[cfg(target_os = "linux")]
pub async fn update_frame_detect_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateFrameDetectReq>,
) -> impl IntoResponse {
    let frame = if req.enabled {
        FRAME_DETECT_INTERVAL
    } else {
        0
    };
    state.kvm.set_frame_detect(frame);
    StatusCode::OK
}

#[cfg(target_os = "linux")]
pub async fn stop_frame_detect_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StopFrameDetectReq>,
) -> impl IntoResponse {
    let duration = req.duration.unwrap_or(10);
    let kvm = state.kvm.clone();

    tokio::spawn(async move {
        kvm.set_frame_detect(0);
        tokio::time::sleep(std::time::Duration::from_secs(duration)).await;
        kvm.set_frame_detect(FRAME_DETECT_INTERVAL);
    });

    StatusCode::OK
}
