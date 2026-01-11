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

#[derive(Debug, Deserialize)]
pub struct SetScreenReq {
    pub stream_type: Option<String>, // "mjpeg" or "h264"
    pub fps: Option<u16>,
    pub quality: Option<u16>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub bitrate: Option<u16>,
    pub gop: Option<u8>,
}

#[cfg(target_os = "linux")]
pub async fn set_screen_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetScreenReq>,
) -> impl IntoResponse {
    let mut config = state.screen_config.write();

    if let Some(stream_type) = req.stream_type {
        config.stream_type = stream_type;
    }
    if let Some(fps) = req.fps {
        config.fps = fps;
    }
    if let Some(quality) = req.quality {
        config.quality = quality;
    }
    if let Some(width) = req.width {
        config.width = width;
    }
    if let Some(height) = req.height {
        config.height = height;
    }
    if let Some(bitrate) = req.bitrate {
        config.bitrate = bitrate;
    }
    if let Some(gop) = req.gop {
        config.gop = gop;
    }

    // Apply settings to KVM hardware
    let kvm = state.kvm.clone();
    if config.stream_type == "h264" {
        kvm.set_stream_type(1); // H264
    } else {
        kvm.set_stream_type(0); // MJPEG
    }

    StatusCode::OK
}

#[cfg(target_os = "linux")]
pub async fn get_screen_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let config = state.screen_config.read();
    Json(config)
}
