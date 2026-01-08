pub mod transport;
pub mod signaling;
pub mod client;
pub mod ws_signaling;
pub mod screen;

use serde::{Deserialize, Serialize};
use bytes::Bytes;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Transport error: {0}")]
    Transport(String),
    #[error("Config error: {0}")]
    Config(String),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("WebRTC error: {0}")]
    WebRtc(#[from] webrtc::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameType {
    H264,
    H265,
    VP8,
    MJPEG,
}

#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub data: Bytes,
    pub frame_type: FrameType,
    pub pts: u64,
}

#[derive(Clone, Debug)]
pub struct H264Frame {
    pub is_keyframe: bool,
    pub timestamp: u64,
    pub packets: Arc<Vec<transport::SharedPacket>>,
    pub raw_data: Bytes,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCapabilities {
    pub supports_h264: bool,
    pub supports_h265: bool,
    pub max_bitrate_kbps: Option<u64>,
}

impl ClientCapabilities {
    pub fn from_sdp(sdp: &str) -> Self {
        let mut caps = Self {
            supports_h264: sdp.contains("H264"),
            supports_h265: sdp.contains("H265") || sdp.contains("HEVC"),
            max_bitrate_kbps: None,
        };
        
        // Simple heuristic for bitrate if present in b=AS:
        if let Some(pos) = sdp.find("b=AS:") {
            let rest = &sdp[pos + 5..];
            if let Some(end) = rest.find("\r\n") {
                if let Ok(val) = rest[..end].parse::<u64>() {
                    caps.max_bitrate_kbps = Some(val);
                }
            }
        }
        
        caps
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterSets {
    pub sps: Bytes,
    pub pps: Bytes,
    pub vps: Option<Bytes>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualityTier {
    Low,
    Medium,
    High,
    Auto,
}
