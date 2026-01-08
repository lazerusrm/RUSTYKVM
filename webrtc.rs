//! WebRTC transport layer for NeuralMatrix
//!
//! Provides WebRTC peer connections for low-latency browser streaming.
//! Supports WHEP (WebRTC-HTTP Egress Protocol) for standardized media egress.
//!
//! # Architecture
//!
//! ```text
//! +----------------+     +------------------+     +----------------+
//! | Video Source   | --> | WebRTC Transport | --> | Browser Client |
//! | (H.264/H.265)  |     | (STUN/TURN/ICE)  |     | (WHEP/SDP)     |
//! +----------------+     +------------------+     +----------------+
//! ```
//!
//! # Features
//!
//! - **ICE/STUN/TURN**: NAT traversal support for peer-to-peer connections
//! - **WHEP Support**: WebRTC-HTTP Egress Protocol for low-latency streaming
//! - **Track Management**: Dynamic video track creation and management
//! - **Signaling Abstractions**: Clean offer/answer/candidate handling
//!
//! # Example
//!
//! ```rust,no_run
//! use nm_transport::webrtc::{
//!     IceServer, PeerConnectionManager, SdpOffer, WebRtcConfig,
//! };
//!
//! async fn example() -> nm_core::Result<()> {
//!     // Configure ICE servers
//!     let config = WebRtcConfig::builder()
//!         .add_stun_server("stun:stun.l.google.com:19302")
//!         .build()?;
//!
//!     // Create peer connection manager
//!     let manager = PeerConnectionManager::new(config).await?;
//!
//!     // Handle incoming offer from browser
//!     let offer_sdp = SdpOffer {
//!         sdp_type: "offer".to_string(),
//!         sdp: "v=0...".to_string(),
//!     };
//!     let (answer, connection_id) = manager
//!         .handle_offer("camera_123", offer_sdp)
//!         .await?;
//!
//!     Ok(())
//! }
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::fragmentation::NalFragmenter;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use bytes::Bytes;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use nm_core::{ClientCapabilities, Error, FrameType, Result, VideoFrame};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, info};
use uuid::Uuid;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264, MIME_TYPE_HEVC, MIME_TYPE_VP8};
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp::packet::Packet as RtpPacket;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};
use webrtc::util::Unmarshal;

// ============================================================================
// Configuration Types
// ============================================================================

/// ICE server configuration for STUN/TURN
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServer {
    /// Server URLs (e.g., "stun:stun.l.google.com:19302", "turn:turn.example.com:3478")
    pub urls: Vec<String>,
    /// Username for TURN authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Credential for TURN authentication
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl IceServer {
    /// Create a STUN server configuration
    pub fn stun(url: impl Into<String>) -> Self {
        Self {
            urls: vec![url.into()],
            username: None,
            credential: None,
        }
    }

    /// Create a TURN server configuration with credentials
    pub fn turn(
        url: impl Into<String>,
        username: impl Into<String>,
        credential: impl Into<String>,
    ) -> Self {
        Self {
            urls: vec![url.into()],
            username: Some(username.into()),
            credential: Some(credential.into()),
        }
    }

    /// Convert to webrtc-rs ICE server
    fn to_rtc_ice_server(&self) -> RTCIceServer {
        RTCIceServer {
            urls: self.urls.clone(),
            username: self.username.clone().unwrap_or_default(),
            credential: self.credential.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
}

/// WebRTC transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRtcConfig {
    /// ICE servers for NAT traversal
    pub ice_servers: Vec<IceServer>,
    /// Enable ICE candidate trickling (default: true)
    #[serde(default = "default_ice_trickle")]
    pub ice_trickle: bool,
    /// ICE connection timeout in seconds
    #[serde(default = "default_ice_timeout")]
    pub ice_connection_timeout_secs: u64,
    /// Maximum number of concurrent peer connections per camera
    #[serde(default = "default_max_peers")]
    pub max_peers_per_camera: usize,
    /// Enable video codec (H.264, H.265, or VP8)
    #[serde(default = "default_video_codec")]
    pub video_codec: VideoCodecType,
    /// RTP payload type for video
    #[serde(default = "default_payload_type")]
    pub video_payload_type: u8,
    /// Video clock rate (90000 for H.264)
    #[serde(default = "default_clock_rate")]
    pub video_clock_rate: u32,
}

fn default_ice_trickle() -> bool {
    true
}
fn default_ice_timeout() -> u64 {
    30
}
fn default_max_peers() -> usize {
    10
}
fn default_video_codec() -> VideoCodecType {
    VideoCodecType::H264
}
fn default_payload_type() -> u8 {
    96
}
fn default_clock_rate() -> u32 {
    90000
}

/// Supported video codecs for WebRTC
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VideoCodecType {
    #[default]
    H264,
    H265,
    VP8,
}

impl WebRtcConfig {
    /// Create a new configuration builder
    pub fn builder() -> WebRtcConfigBuilder {
        WebRtcConfigBuilder::default()
    }

    /// Create default configuration with Google STUN servers
    pub fn default_with_stun() -> Self {
        Self {
            ice_servers: vec![
                IceServer::stun("stun:stun.l.google.com:19302"),
                IceServer::stun("stun:stun1.l.google.com:19302"),
            ],
            ice_trickle: true,
            ice_connection_timeout_secs: 30,
            max_peers_per_camera: 10,
            video_codec: VideoCodecType::H264,
            video_payload_type: 96,
            video_clock_rate: 90000,
        }
    }

    /// Get RTCConfiguration for webrtc-rs
    fn to_rtc_configuration(&self) -> RTCConfiguration {
        RTCConfiguration {
            ice_servers: self
                .ice_servers
                .iter()
                .map(|s| s.to_rtc_ice_server())
                .collect(),
            ..Default::default()
        }
    }
}

/// Builder for WebRTC configuration
#[derive(Debug, Default)]
pub struct WebRtcConfigBuilder {
    ice_servers: Vec<IceServer>,
    ice_trickle: Option<bool>,
    ice_timeout: Option<u64>,
    max_peers: Option<usize>,
    video_codec: Option<VideoCodecType>,
}

impl WebRtcConfigBuilder {
    /// Add a STUN server
    pub fn add_stun_server(mut self, url: impl Into<String>) -> Self {
        self.ice_servers.push(IceServer::stun(url));
        self
    }

    /// Add a TURN server with credentials
    pub fn add_turn_server(
        mut self,
        url: impl Into<String>,
        username: impl Into<String>,
        credential: impl Into<String>,
    ) -> Self {
        self.ice_servers
            .push(IceServer::turn(url, username, credential));
        self
    }

    /// Add a custom ICE server
    pub fn add_ice_server(mut self, server: IceServer) -> Self {
        self.ice_servers.push(server);
        self
    }

    /// Enable or disable ICE trickle
    pub fn ice_trickle(mut self, enabled: bool) -> Self {
        self.ice_trickle = Some(enabled);
        self
    }

    /// Set ICE connection timeout
    pub fn ice_timeout_secs(mut self, secs: u64) -> Self {
        self.ice_timeout = Some(secs);
        self
    }

    /// Set maximum peers per camera
    pub fn max_peers_per_camera(mut self, max: usize) -> Self {
        self.max_peers = Some(max);
        self
    }

    /// Set video codec type
    pub fn video_codec(mut self, codec: VideoCodecType) -> Self {
        self.video_codec = Some(codec);
        self
    }

    /// Build the configuration
    pub fn build(self) -> Result<WebRtcConfig> {
        let ice_servers = if self.ice_servers.is_empty() {
            // Default STUN servers
            vec![
                IceServer::stun("stun:stun.l.google.com:19302"),
                IceServer::stun("stun:stun1.l.google.com:19302"),
            ]
        } else {
            self.ice_servers
        };

        Ok(WebRtcConfig {
            ice_servers,
            ice_trickle: self.ice_trickle.unwrap_or(true),
            ice_connection_timeout_secs: self.ice_timeout.unwrap_or(30),
            max_peers_per_camera: self.max_peers.unwrap_or(10),
            video_codec: self.video_codec.unwrap_or(VideoCodecType::H264),
            video_payload_type: 96,
            video_clock_rate: 90000,
        })
    }
}

// ============================================================================
// Signaling Types
// ============================================================================

/// SDP offer from a browser client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdpOffer {
    /// SDP type (always "offer")
    #[serde(rename = "type")]
    pub sdp_type: String,
    /// SDP content
    pub sdp: String,
}

/// SDP answer to send to browser
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdpAnswer {
    /// SDP type (always "answer")
    #[serde(rename = "type")]
    pub sdp_type: String,
    /// SDP content
    pub sdp: String,
}

/// ICE candidate for NAT traversal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceCandidate {
    /// Candidate string
    pub candidate: String,
    /// SDP media description index
    #[serde(rename = "sdpMLineIndex")]
    pub sdp_m_line_index: Option<u16>,
    /// SDP media ID
    #[serde(rename = "sdpMid")]
    pub sdp_mid: Option<String>,
    /// Username fragment
    #[serde(rename = "usernameFragment")]
    pub username_fragment: Option<String>,
}

impl IceCandidate {
    /// Convert to webrtc-rs ICE candidate init
    fn to_rtc_candidate_init(&self) -> RTCIceCandidateInit {
        RTCIceCandidateInit {
            candidate: self.candidate.clone(),
            sdp_mid: self.sdp_mid.clone(),
            sdp_mline_index: self.sdp_m_line_index,
            username_fragment: self.username_fragment.clone(),
        }
    }
}

/// Signaling message types for WebRTC negotiation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SignalingMessage {
    /// SDP offer from browser
    Offer(SdpOffer),
    /// SDP answer from edge
    Answer(SdpAnswer),
    /// ICE candidate exchange
    IceCandidate(IceCandidate),
    /// Connection error
    Error { message: String },
}

// ============================================================================
// Peer Connection State
// ============================================================================

/// State of a peer connection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    /// Connection is being established
    Connecting,
    /// Connection is active and streaming
    Connected,
    /// Connection is temporarily disconnected
    Disconnected,
    /// Connection failed permanently
    Failed,
    /// Connection was closed
    Closed,
}

impl From<RTCPeerConnectionState> for ConnectionState {
    fn from(state: RTCPeerConnectionState) -> Self {
        match state {
            RTCPeerConnectionState::New | RTCPeerConnectionState::Connecting => {
                ConnectionState::Connecting
            }
            RTCPeerConnectionState::Connected => ConnectionState::Connected,
            RTCPeerConnectionState::Disconnected => ConnectionState::Disconnected,
            RTCPeerConnectionState::Failed => ConnectionState::Failed,
            RTCPeerConnectionState::Closed => ConnectionState::Closed,
            _ => ConnectionState::Disconnected,
        }
    }
}

/// Information about an active peer connection
#[derive(Debug, Clone)]
pub struct PeerInfo {
    /// Unique connection identifier
    pub connection_id: String,
    /// Camera ID this connection is streaming from
    pub camera_id: String,
    /// Current connection state
    pub state: ConnectionState,
    /// ICE connection state
    pub ice_state: RTCIceConnectionState,
    /// When the connection was created
    pub created_at: std::time::Instant,
}

// ============================================================================
// Internal Connection State
// ============================================================================

/// Internal state for a peer connection (stored in manager)
#[allow(dead_code)]
struct InternalConnection {
    /// Unique connection ID
    connection_id: String,
    /// Camera ID being streamed
    camera_id: String,
    /// When the connection was created
    created_at: std::time::Instant,
    /// The peer connection
    peer_connection: Arc<RTCPeerConnection>,
    /// Video track for sending frames
    video_track: Arc<TrackLocalStaticRTP>,
    /// State broadcast sender (kept alive to send state updates)
    state_tx: broadcast::Sender<ConnectionState>,
    /// Negotiated video codec for this connection
    video_codec: VideoCodecType,
    /// Client codec and bandwidth capabilities
    client_caps: ClientCapabilities,
    /// RTP sequence number for broadcast path
    sequence_number: u16,
    /// RTP SSRC for broadcast path
    ssrc: u32,
}

// ============================================================================
// Peer Connection Handle
// ============================================================================

/// Handle to an active peer connection
///
/// This handle is returned to the caller and provides methods for interacting
/// with the WebRTC peer connection.
pub struct PeerConnectionHandle {
    /// Unique connection ID
    pub connection_id: String,
    /// Camera ID being streamed
    pub camera_id: String,
    /// The peer connection (shared reference)
    peer_connection: Arc<RTCPeerConnection>,
    /// Video track for sending frames (shared reference)
    video_track: Arc<TrackLocalStaticRTP>,
    /// Channel to receive state updates
    state_rx: broadcast::Receiver<ConnectionState>,
    /// Channel to receive local ICE candidates
    ice_candidate_rx: mpsc::UnboundedReceiver<IceCandidate>,
    /// RTP sequence number
    sequence_number: u16,
    /// RTP timestamp
    timestamp: u32,
    /// RTP SSRC
    ssrc: u32,
}

impl PeerConnectionHandle {
    /// Get current connection state
    pub fn state(&self) -> ConnectionState {
        self.peer_connection.connection_state().into()
    }

    /// Check if connection is active
    pub fn is_active(&self) -> bool {
        matches!(
            self.state(),
            ConnectionState::Connected | ConnectionState::Connecting
        )
    }

    /// Get the connection ID
    pub fn id(&self) -> &str {
        &self.connection_id
    }

    /// Send a video frame over WebRTC
    ///
    /// The frame should contain H.264/H.265 NAL units in Annex B format.
    /// This method will packetize them into RTP packets for WebRTC transport.
    pub async fn send_video_frame(&mut self, frame: &VideoFrame) -> Result<()> {
        if !self.is_active() {
            return Err(Error::Transport("Connection not active".into()));
        }

        // Convert frame data to RTP packets
        let packet_bytes = match frame.frame_type {
            FrameType::H264 => {
                packetize_h264(&frame.data, self.sequence_number, self.timestamp, self.ssrc)?
            }
            FrameType::H265 => {
                packetize_h265(&frame.data, self.sequence_number, self.timestamp, self.ssrc)?
            }
            _ => {
                return Err(Error::Transport(format!(
                    "Unsupported video frame type: {:?}",
                    frame.frame_type
                )));
            }
        };

        for bytes in packet_bytes {
            let packet = RtpPacket::unmarshal(&mut bytes.as_ref())
                .map_err(|e| Error::Transport(format!("Failed to parse RTP packet: {}", e)))?;
            self.video_track
                .write_rtp(&packet)
                .await
                .map_err(|e| Error::Transport(format!("Failed to write RTP packet: {}", e)))?;
            self.sequence_number = self.sequence_number.wrapping_add(1);
        }

        // Increment timestamp by frame duration (assuming 30fps -> 3000 @ 90kHz clock)
        self.timestamp = self.timestamp.wrapping_add(3000);

        Ok(())
    }

    /// Send raw RTP packet data
    pub async fn send_rtp(&mut self, rtp_data: &[u8]) -> Result<()> {
        if !self.is_active() {
            return Err(Error::Transport("Connection not active".into()));
        }

        // Parse RTP packet from raw bytes
        let packet = RtpPacket::unmarshal(&mut rtp_data.to_vec().as_slice())
            .map_err(|e| Error::Transport(format!("Failed to parse RTP: {}", e)))?;
        self.video_track
            .write_rtp(&packet)
            .await
            .map_err(|e| Error::Transport(format!("Failed to write RTP: {}", e)))?;

        Ok(())
    }

    /// Add a remote ICE candidate
    pub async fn add_ice_candidate(&self, candidate: IceCandidate) -> Result<()> {
        self.peer_connection
            .add_ice_candidate(candidate.to_rtc_candidate_init())
            .await
            .map_err(|e| Error::Transport(format!("Failed to add ICE candidate: {}", e)))?;

        debug!(connection_id = %self.connection_id, "Added remote ICE candidate");
        Ok(())
    }

    /// Get the next local ICE candidate (for trickle ICE)
    pub async fn next_ice_candidate(&mut self) -> Option<IceCandidate> {
        self.ice_candidate_rx.next().await
    }

    /// Subscribe to connection state changes
    pub fn subscribe_state(&self) -> broadcast::Receiver<ConnectionState> {
        self.state_rx.resubscribe()
    }

    /// Close the peer connection
    pub async fn close(&self) -> Result<()> {
        self.peer_connection
            .close()
            .await
            .map_err(|e| Error::Transport(format!("Failed to close connection: {}", e)))?;

        info!(connection_id = %self.connection_id, "Peer connection closed");
        Ok(())
    }
}

// ============================================================================
// Peer Connection Manager
// ============================================================================

/// Manages WebRTC peer connections for multiple cameras
pub struct PeerConnectionManager {
    /// Configuration
    config: WebRtcConfig,
    /// WebRTC API instance
    api: webrtc::api::API,
    /// Active connections by connection ID
    connections: Arc<RwLock<HashMap<String, InternalConnection>>>,
    /// Connections grouped by camera ID
    camera_connections: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Broadcast channel for global events
    event_tx: broadcast::Sender<PeerEvent>,
    /// Cached H.264 parameter sets per camera (for SDP sprop-parameter-sets)
    h264_parameter_sets: Arc<RwLock<HashMap<String, H264ParameterSets>>>,
    /// Cached H.265 parameter sets per camera (for SDP sprop-parameter-sets)
    h265_parameter_sets: Arc<RwLock<HashMap<String, H265ParameterSets>>>,
    /// Preferred video codec per camera (inferred from stream)
    camera_codecs: Arc<RwLock<HashMap<String, VideoCodecType>>>,
}

#[derive(Debug, Clone, Default)]
pub struct H264ParameterSets {
    pub sps: Bytes,
    pub pps: Bytes,
}

#[derive(Debug, Clone, Default)]
pub struct H265ParameterSets {
    pub vps: Bytes,
    pub sps: Bytes,
    pub pps: Bytes,
}

/// Events emitted by the peer connection manager
#[derive(Debug, Clone)]
pub enum PeerEvent {
    /// New peer connected
    Connected {
        connection_id: String,
        camera_id: String,
    },
    /// Peer disconnected
    Disconnected {
        connection_id: String,
        camera_id: String,
    },
    /// ICE candidate gathered
    IceCandidate {
        connection_id: String,
        candidate: IceCandidate,
    },
    /// Connection state changed
    StateChanged {
        connection_id: String,
        state: ConnectionState,
    },
}

impl PeerConnectionManager {
    /// Create a new peer connection manager
    pub async fn new(config: WebRtcConfig) -> Result<Self> {
        // Create media engine with H.264 support
        let mut media_engine = MediaEngine::default();

        match config.video_codec {
            VideoCodecType::H264 => {
                media_engine
                    .register_default_codecs()
                    .map_err(|e| Error::Config(format!("Failed to register codecs: {}", e)))?;
            }
            VideoCodecType::H265 => {
                media_engine
                    .register_default_codecs()
                    .map_err(|e| Error::Config(format!("Failed to register codecs: {}", e)))?;
            }
            VideoCodecType::VP8 => {
                media_engine
                    .register_default_codecs()
                    .map_err(|e| Error::Config(format!("Failed to register codecs: {}", e)))?;
            }
        }

        // Create interceptor registry for RTP/RTCP handling
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)
            .map_err(|e| Error::Config(format!("Failed to register interceptors: {}", e)))?;

        // Create setting engine
        let setting_engine = SettingEngine::default();

        // Build API
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_setting_engine(setting_engine)
            .build();

        let (event_tx, _) = broadcast::channel(256);

        info!(
            ice_servers = config.ice_servers.len(),
            video_codec = ?config.video_codec,
            "WebRTC peer connection manager initialized"
        );

        Ok(Self {
            config,
            api,
            connections: Arc::new(RwLock::new(HashMap::new())),
            camera_connections: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            h264_parameter_sets: Arc::new(RwLock::new(HashMap::new())),
            h265_parameter_sets: Arc::new(RwLock::new(HashMap::new())),
            camera_codecs: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Update cached H.264 parameter sets for a camera.
    pub fn update_h264_parameter_sets(&self, camera_id: &str, sps: Bytes, pps: Bytes) {
        if sps.is_empty() || pps.is_empty() {
            return;
        }
        self.h264_parameter_sets
            .write()
            .insert(camera_id.to_string(), H264ParameterSets { sps, pps });
    }

    /// Update cached H.265 parameter sets for a camera.
    pub fn update_h265_parameter_sets(&self, camera_id: &str, vps: Bytes, sps: Bytes, pps: Bytes) {
        if vps.is_empty() || sps.is_empty() || pps.is_empty() {
            return;
        }
        self.h265_parameter_sets
            .write()
            .insert(camera_id.to_string(), H265ParameterSets { vps, sps, pps });
    }

    /// Record preferred codec for a camera based on observed stream frames.
    pub fn set_camera_codec(&self, camera_id: &str, codec: VideoCodecType) {
        self.camera_codecs
            .write()
            .insert(camera_id.to_string(), codec);
    }

    /// Handle an SDP offer from a browser client
    ///
    /// Returns the SDP answer and a handle to interact with the peer connection.
    pub async fn handle_offer(
        &self,
        camera_id: &str,
        offer: SdpOffer,
    ) -> Result<(SdpAnswer, PeerConnectionHandle)> {
        // Check connection limit
        let current_count = self.connection_count_for_camera(camera_id);
        if current_count >= self.config.max_peers_per_camera {
            return Err(Error::Transport(format!(
                "Maximum peers ({}) reached for camera {}",
                self.config.max_peers_per_camera, camera_id
            )));
        }

        // Create peer connection
        let peer_connection = Arc::new(
            self.api
                .new_peer_connection(self.config.to_rtc_configuration())
                .await
                .map_err(|e| {
                    Error::Transport(format!("Failed to create peer connection: {}", e))
                })?,
        );

        let connection_id = Uuid::new_v4().to_string();

        // Decide codec based on stream and client capabilities
        let stream_codec = self
            .camera_codecs
            .read()
            .get(camera_id)
            .copied()
            .unwrap_or(self.config.video_codec);
        let client_caps = ClientCapabilities::from_sdp(&offer.sdp);
        let selected_codec = match stream_codec {
            VideoCodecType::H265 if client_caps.supports_h265 => VideoCodecType::H265,
            VideoCodecType::H265 => VideoCodecType::H264,
            VideoCodecType::H264 => VideoCodecType::H264,
            VideoCodecType::VP8 => VideoCodecType::VP8,
        };
        if selected_codec == VideoCodecType::H264 && !client_caps.supports_h264 {
            return Err(Error::Transport(
                "Client does not support H264; cannot deliver stream without transcoding".into(),
            ));
        }
        if selected_codec == VideoCodecType::H265 && !client_caps.supports_h265 {
            return Err(Error::Transport(
                "Client does not support H265; cannot deliver stream".into(),
            ));
        }
        let fmtp_line = match selected_codec {
            VideoCodecType::H264 => self.h264_fmtp_line(camera_id).unwrap_or_default(),
            VideoCodecType::H265 => self.h265_fmtp_line(camera_id).unwrap_or_default(),
            VideoCodecType::VP8 => String::new(),
        };
        let video_track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: match selected_codec {
                    VideoCodecType::H264 => MIME_TYPE_H264.to_string(),
                    VideoCodecType::H265 => MIME_TYPE_HEVC.to_string(),
                    VideoCodecType::VP8 => MIME_TYPE_VP8.to_string(),
                },
                clock_rate: self.config.video_clock_rate,
                channels: 0,
                sdp_fmtp_line: fmtp_line,
                rtcp_feedback: vec![],
            },
            format!("video-{}", connection_id),
            format!("stream-{}", camera_id),
        ));

        // Add track to peer connection
        let rtp_sender = peer_connection
            .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| Error::Transport(format!("Failed to add track: {}", e)))?;

        // Spawn task to handle RTCP packets (for congestion control feedback)
        let rtp_sender_clone = rtp_sender.clone();
        tokio::spawn(async move {
            let mut rtcp_buf = vec![0u8; 1500];
            while let Ok((_, _)) = rtp_sender_clone.read(&mut rtcp_buf).await {
                // RTCP packets handled automatically by interceptors
            }
        });

        // Create channels for state updates and ICE candidates
        let (state_tx, state_rx) = broadcast::channel(16);
        let (ice_tx, ice_rx) = mpsc::unbounded();

        // Set up ICE candidate handler
        let conn_id = connection_id.clone();
        let event_tx = self.event_tx.clone();
        peer_connection.on_ice_candidate(Box::new(move |candidate| {
            let conn_id = conn_id.clone();
            let mut ice_tx = ice_tx.clone();
            let event_tx = event_tx.clone();

            Box::pin(async move {
                if let Some(c) = candidate {
                    // Convert to JSON to get SDP info
                    let json = c.to_json().unwrap_or_default();
                    let ice_candidate = IceCandidate {
                        candidate: json.candidate,
                        sdp_m_line_index: json.sdp_mline_index,
                        sdp_mid: json.sdp_mid,
                        username_fragment: json.username_fragment,
                    };

                    let _ = ice_tx.send(ice_candidate.clone()).await;
                    let _ = event_tx.send(PeerEvent::IceCandidate {
                        connection_id: conn_id,
                        candidate: ice_candidate,
                    });
                }
            })
        }));

        // Set up connection state handler
        let conn_id = connection_id.clone();
        let cam_id = camera_id.to_string();
        let event_tx = self.event_tx.clone();
        let state_tx_clone = state_tx.clone();
        peer_connection.on_peer_connection_state_change(Box::new(move |state| {
            let conn_id = conn_id.clone();
            let cam_id = cam_id.clone();
            let event_tx = event_tx.clone();
            let state_tx = state_tx_clone.clone();

            Box::pin(async move {
                let conn_state = ConnectionState::from(state);
                let _ = state_tx.send(conn_state);

                match conn_state {
                    ConnectionState::Connected => {
                        info!(connection_id = %conn_id, camera_id = %cam_id, "Peer connected");
                        let _ = event_tx.send(PeerEvent::Connected {
                            connection_id: conn_id,
                            camera_id: cam_id,
                        });
                    }
                    ConnectionState::Disconnected | ConnectionState::Failed | ConnectionState::Closed => {
                        info!(connection_id = %conn_id, camera_id = %cam_id, state = ?conn_state, "Peer disconnected");
                        let _ = event_tx.send(PeerEvent::Disconnected {
                            connection_id: conn_id,
                            camera_id: cam_id,
                        });
                    }
                    _ => {}
                }
            })
        }));

        // Set remote description (offer)
        let rtc_offer = RTCSessionDescription::offer(offer.sdp)
            .map_err(|e| Error::Transport(format!("Invalid offer SDP: {}", e)))?;

        peer_connection
            .set_remote_description(rtc_offer)
            .await
            .map_err(|e| Error::Transport(format!("Failed to set remote description: {}", e)))?;

        // Create answer
        let answer = peer_connection
            .create_answer(None)
            .await
            .map_err(|e| Error::Transport(format!("Failed to create answer: {}", e)))?;

        // Set local description
        peer_connection
            .set_local_description(answer.clone())
            .await
            .map_err(|e| Error::Transport(format!("Failed to set local description: {}", e)))?;

        // Wait for ICE gathering to complete if not trickling
        if !self.config.ice_trickle {
            // Wait a bit for ICE gathering
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        // Get final SDP
        let local_desc = peer_connection
            .local_description()
            .await
            .ok_or_else(|| Error::Transport("No local description".into()))?;

        let sdp_answer = SdpAnswer {
            sdp_type: "answer".to_string(),
            sdp: local_desc.sdp,
        };

        // Store internal connection
        let internal_conn = InternalConnection {
            connection_id: connection_id.clone(),
            camera_id: camera_id.to_string(),
            created_at: std::time::Instant::now(),
            peer_connection: Arc::clone(&peer_connection),
            video_track: Arc::clone(&video_track),
            state_tx,
            video_codec: selected_codec,
            client_caps: client_caps.clone(),
            sequence_number: 0,
            ssrc: rand::random::<u32>(),
        };

        {
            let mut connections = self.connections.write();
            connections.insert(connection_id.clone(), internal_conn);
        }
        {
            let mut camera_conns = self.camera_connections.write();
            camera_conns
                .entry(camera_id.to_string())
                .or_insert_with(Vec::new)
                .push(connection_id.clone());
        }

        info!(
            connection_id = %connection_id,
            camera_id = %camera_id,
            "WebRTC peer connection created"
        );

        // Create and return handle for the caller
        let handle = PeerConnectionHandle {
            connection_id: connection_id.clone(),
            camera_id: camera_id.to_string(),
            peer_connection,
            video_track,
            state_rx,
            ice_candidate_rx: ice_rx,
            sequence_number: 0,
            timestamp: 0,
            ssrc: rand::random::<u32>(),
        };

        Ok((sdp_answer, handle))
    }

    fn h264_fmtp_line(&self, camera_id: &str) -> Option<String> {
        let sets = self.h264_parameter_sets.read().get(camera_id).cloned()?;
        let sps = strip_annex_b(&sets.sps);
        let pps = strip_annex_b(&sets.pps);
        if sps.is_empty() || pps.is_empty() {
            return None;
        }

        let sprop = format!(
            "{},{}",
            BASE64_STANDARD.encode(sps),
            BASE64_STANDARD.encode(pps)
        );
        let mut parts = vec!["packetization-mode=1".to_string()];
        if let Some(profile) = h264_profile_level_id(sps) {
            parts.push(format!("profile-level-id={}", profile));
        }
        parts.push(format!("sprop-parameter-sets={}", sprop));
        Some(parts.join(";"))
    }

    fn h265_fmtp_line(&self, camera_id: &str) -> Option<String> {
        let sets = self.h265_parameter_sets.read().get(camera_id).cloned()?;
        let vps = strip_annex_b(&sets.vps);
        let sps = strip_annex_b(&sets.sps);
        let pps = strip_annex_b(&sets.pps);
        if vps.is_empty() || sps.is_empty() || pps.is_empty() {
            return None;
        }

        Some(format!(
            "sprop-vps={};sprop-sps={};sprop-pps={}",
            BASE64_STANDARD.encode(vps),
            BASE64_STANDARD.encode(sps),
            BASE64_STANDARD.encode(pps),
        ))
    }

    /// Get connection count for a specific camera
    pub fn connection_count_for_camera(&self, camera_id: &str) -> usize {
        self.camera_connections
            .read()
            .get(camera_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Get total connection count
    pub fn total_connection_count(&self) -> usize {
        self.connections.read().len()
    }

    /// Get all active connection IDs for a camera
    pub fn get_camera_connections(&self, camera_id: &str) -> Vec<String> {
        self.camera_connections
            .read()
            .get(camera_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Remove a connection
    pub async fn remove_connection(&self, connection_id: &str) -> Result<()> {
        let handle = {
            let mut connections = self.connections.write();
            connections.remove(connection_id)
        };

        if let Some(handle) = handle {
            // Remove from camera connections
            {
                let mut camera_conns = self.camera_connections.write();
                if let Some(conns) = camera_conns.get_mut(&handle.camera_id) {
                    conns.retain(|id| id != connection_id);
                }
            }

            // Close the peer connection
            handle
                .peer_connection
                .close()
                .await
                .map_err(|e| Error::Transport(format!("Failed to close connection: {}", e)))?;

            info!(connection_id = %connection_id, "Connection removed");
        }

        Ok(())
    }

    /// Subscribe to peer events
    pub fn subscribe_events(&self) -> broadcast::Receiver<PeerEvent> {
        self.event_tx.subscribe()
    }

    /// Add an ICE candidate to a specific connection
    pub async fn add_ice_candidate(
        &self,
        connection_id: &str,
        candidate: IceCandidate,
    ) -> Result<()> {
        let peer_connection = {
            let connections = self.connections.read();
            connections
                .get(connection_id)
                .map(|c| Arc::clone(&c.peer_connection))
        };

        let peer_connection = peer_connection
            .ok_or_else(|| Error::Transport(format!("Connection {} not found", connection_id)))?;

        peer_connection
            .add_ice_candidate(candidate.to_rtc_candidate_init())
            .await
            .map_err(|e| Error::Transport(format!("Failed to add ICE candidate: {}", e)))?;

        debug!(connection_id = %connection_id, "Added remote ICE candidate via manager");
        Ok(())
    }

    /// Broadcast a video frame to all connections for a camera
    ///
    /// This is a convenience method for sending frames to all viewers of a camera.
    /// For more control, use the PeerConnectionHandle returned from handle_offer.
    pub async fn broadcast_frame(&self, camera_id: &str, frame: &VideoFrame) -> Result<usize> {
        let connection_ids = self.get_camera_connections(camera_id);
        let mut sent_count = 0;

        for conn_id in connection_ids {
            if self.send_frame_to_connection(&conn_id, frame).await? {
                sent_count += 1;
            }
        }

        Ok(sent_count)
    }

    /// Send a frame to a specific connection by ID.
    pub async fn send_frame_to_connection(
        &self,
        connection_id: &str,
        frame: &VideoFrame,
    ) -> Result<bool> {
        let (track, mut sequence_number, ssrc) = {
            let mut connections = self.connections.write();
            let connection = match connections.get_mut(connection_id) {
                Some(connection) => connection,
                None => return Ok(false),
            };
            (
                Arc::clone(&connection.video_track),
                connection.sequence_number,
                connection.ssrc,
            )
        };

        let track_mime = track.codec().mime_type;
        let expected_mime = match frame.frame_type {
            FrameType::H264 => MIME_TYPE_H264,
            FrameType::H265 => MIME_TYPE_HEVC,
            _ => "",
        };
        if !expected_mime.is_empty() && !track_mime.eq_ignore_ascii_case(expected_mime) {
            debug!(
                connection_id = %connection_id,
                track_mime = %track_mime,
                frame_type = ?frame.frame_type,
                "Skipping frame: track codec does not match frame type"
            );
            return Ok(false);
        }

        let timestamp = frame.pts as u32;
        let packet_bytes = match frame.frame_type {
            FrameType::H264 => packetize_h264(&frame.data, sequence_number, timestamp, ssrc)?,
            FrameType::H265 => packetize_h265(&frame.data, sequence_number, timestamp, ssrc)?,
            _ => Vec::new(),
        };
        for bytes in &packet_bytes {
            if let Ok(packet) = RtpPacket::unmarshal(&mut bytes.as_ref()) {
                if track.write_rtp(&packet).await.is_ok() {
                    sequence_number = sequence_number.wrapping_add(1);
                }
            }
        }

        if let Some(connection) = self.connections.write().get_mut(connection_id) {
            connection.sequence_number = sequence_number;
        }

        Ok(!packet_bytes.is_empty())
    }

    /// Get active connections and negotiated codecs for a camera.
    pub fn get_camera_connections_with_codecs(
        &self,
        camera_id: &str,
    ) -> Vec<(String, VideoCodecType)> {
        let connection_ids = self.get_camera_connections(camera_id);
        let connections = self.connections.read();
        connection_ids
            .into_iter()
            .filter_map(|id| connections.get(&id).map(|conn| (id, conn.video_codec)))
            .collect()
    }

    /// Get active connections with codec and max bitrate hints.
    pub fn get_camera_connections_with_caps(
        &self,
        camera_id: &str,
    ) -> Vec<(String, VideoCodecType, Option<u64>)> {
        let connection_ids = self.get_camera_connections(camera_id);
        let connections = self.connections.read();
        connection_ids
            .into_iter()
            .filter_map(|id| {
                connections
                    .get(&id)
                    .map(|conn| (id, conn.video_codec, conn.client_caps.max_bitrate_kbps))
            })
            .collect()
    }

    /// Get connection info by ID
    pub fn get_connection_info(&self, connection_id: &str) -> Option<PeerInfo> {
        let connections = self.connections.read();
        connections.get(connection_id).map(|c| PeerInfo {
            connection_id: c.connection_id.clone(),
            camera_id: c.camera_id.clone(),
            state: c.peer_connection.connection_state().into(),
            ice_state: c.peer_connection.ice_connection_state(),
            created_at: c.created_at,
        })
    }
}

// ============================================================================
// WHEP (WebRTC-HTTP Egress Protocol) Support
// ============================================================================

/// WHEP endpoint for standardized WebRTC egress
///
/// WHEP provides a standard HTTP-based protocol for setting up WebRTC streaming
/// without custom signaling servers. This is ideal for low-latency video streaming.
pub struct WhepEndpoint {
    /// Peer connection manager
    manager: Arc<PeerConnectionManager>,
    /// Active WHEP resources by resource ID
    resources: Arc<RwLock<HashMap<String, WhepResource>>>,
}

/// A WHEP resource representing an active stream
#[derive(Debug, Clone)]
pub struct WhepResource {
    /// Resource ID (used in WHEP URLs)
    pub resource_id: String,
    /// Connection ID
    pub connection_id: String,
    /// Camera ID being streamed
    pub camera_id: String,
    /// ETag for conditional requests
    pub etag: String,
}

/// WHEP request for creating a new resource
#[derive(Debug, Clone)]
pub struct WhepRequest {
    /// Camera ID to stream
    pub camera_id: String,
    /// SDP offer from client
    pub offer_sdp: String,
}

/// WHEP response containing the answer
#[derive(Debug, Clone)]
pub struct WhepResponse {
    /// Resource ID for DELETE/PATCH operations
    pub resource_id: String,
    /// SDP answer
    pub answer_sdp: String,
    /// Location header value
    pub location: String,
    /// ETag for conditional requests
    pub etag: String,
}

impl WhepEndpoint {
    /// Create a new WHEP endpoint
    pub fn new(manager: Arc<PeerConnectionManager>) -> Self {
        Self {
            manager,
            resources: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Handle WHEP POST request (create resource)
    ///
    /// This implements the WHEP protocol for initiating a WebRTC stream.
    /// Returns an SDP answer that the client should use to complete the connection.
    pub async fn create_resource(&self, request: WhepRequest) -> Result<WhepResponse> {
        let offer = SdpOffer {
            sdp_type: "offer".to_string(),
            sdp: request.offer_sdp,
        };

        let (answer, handle) = self.manager.handle_offer(&request.camera_id, offer).await?;

        let resource_id = Uuid::new_v4().to_string();
        let etag = format!("\"{}\"", Uuid::new_v4());

        let resource = WhepResource {
            resource_id: resource_id.clone(),
            connection_id: handle.connection_id.clone(),
            camera_id: request.camera_id.clone(),
            etag: etag.clone(),
        };

        self.resources.write().insert(resource_id.clone(), resource);

        info!(
            resource_id = %resource_id,
            camera_id = %request.camera_id,
            "WHEP resource created"
        );

        Ok(WhepResponse {
            resource_id: resource_id.clone(),
            answer_sdp: answer.sdp,
            location: format!("/whep/resource/{}", resource_id),
            etag,
        })
    }

    /// Handle WHEP PATCH request (trickle ICE)
    pub async fn add_ice_candidate(
        &self,
        resource_id: &str,
        candidate: IceCandidate,
    ) -> Result<()> {
        let resource = {
            let resources = self.resources.read();
            resources.get(resource_id).cloned()
        };

        let resource = resource.ok_or_else(|| Error::Transport("Resource not found".into()))?;

        let peer_connection = {
            let connections = self.manager.connections.read();
            connections
                .get(&resource.connection_id)
                .map(|conn| Arc::clone(&conn.peer_connection))
        };

        if let Some(peer_connection) = peer_connection {
            peer_connection
                .add_ice_candidate(candidate.to_rtc_candidate_init())
                .await
                .map_err(|e| Error::Transport(format!("Failed to add ICE candidate: {}", e)))?;
        }

        Ok(())
    }

    /// Handle WHEP DELETE request (terminate resource)
    pub async fn delete_resource(&self, resource_id: &str) -> Result<()> {
        let resource = {
            let mut resources = self.resources.write();
            resources.remove(resource_id)
        };

        if let Some(resource) = resource {
            self.manager
                .remove_connection(&resource.connection_id)
                .await?;
            info!(resource_id = %resource_id, "WHEP resource deleted");
        }

        Ok(())
    }

    /// Get resource by ID
    pub fn get_resource(&self, resource_id: &str) -> Option<WhepResource> {
        self.resources.read().get(resource_id).cloned()
    }

    /// List all active resources
    pub fn list_resources(&self) -> Vec<WhepResource> {
        self.resources.read().values().cloned().collect()
    }
}

// ============================================================================
// RTP Packetization Helpers
// ============================================================================

/// Maximum RTP payload size (MTU minus headers)
const MAX_RTP_PAYLOAD_SIZE: usize = 1200;

/// Packetize H.264 NAL units into RTP packets
///
/// Handles both single NAL unit mode and FU-A fragmentation for large NAL units.
fn packetize_h264(data: &[u8], mut seq: u16, timestamp: u32, ssrc: u32) -> Result<Vec<Bytes>> {
    let mut packets = Vec::new();

    // Find NAL units in the data
    let nal_units = find_nal_units(data);

    for (nal_index, nal) in nal_units.iter().enumerate() {
        let is_last_nal = nal_index + 1 == nal_units.len();
        if nal.len() <= MAX_RTP_PAYLOAD_SIZE {
            // Single NAL unit mode - fits in one packet
            let mut packet = Vec::with_capacity(12 + nal.len());

            // RTP header (12 bytes)
            packet.push(0x80); // V=2, P=0, X=0, CC=0
            let marker = if is_last_nal { 0x80 } else { 0x00 };
            packet.push(marker | 96); // M=marker, PT=96 (dynamic)
            packet.extend_from_slice(&seq.to_be_bytes());
            packet.extend_from_slice(&timestamp.to_be_bytes());
            packet.extend_from_slice(&ssrc.to_be_bytes());

            // NAL unit data
            packet.extend_from_slice(nal);

            packets.push(Bytes::from(packet));
            seq = seq.wrapping_add(1);
        } else {
            // FU-A fragmentation for large NAL units
            let nal_header = nal[0];
            let nal_type = nal_header & 0x1F;
            let nri = nal_header & 0x60;

            let payload = &nal[1..]; // Skip NAL header
            let mut offset = 0;
            let mut first = true;

            while offset < payload.len() {
                let remaining = payload.len() - offset;
                let chunk_size = std::cmp::min(remaining, MAX_RTP_PAYLOAD_SIZE - 2);
                let last = offset + chunk_size >= payload.len();

                let mut packet = Vec::with_capacity(12 + 2 + chunk_size);

                // RTP header
                let marker = if last && is_last_nal { 0x80 } else { 0x00 };
                packet.push(0x80);
                packet.push(marker | 96);
                packet.extend_from_slice(&seq.to_be_bytes());
                packet.extend_from_slice(&timestamp.to_be_bytes());
                packet.extend_from_slice(&ssrc.to_be_bytes());

                // FU indicator (NAL type 28 = FU-A)
                packet.push(nri | 28);

                // FU header
                let fu_header = if first {
                    0x80 | nal_type // Start bit
                } else if last {
                    0x40 | nal_type // End bit
                } else {
                    nal_type
                };
                packet.push(fu_header);

                // Payload fragment
                packet.extend_from_slice(&payload[offset..offset + chunk_size]);

                packets.push(Bytes::from(packet));
                seq = seq.wrapping_add(1);
                offset += chunk_size;
                first = false;
            }
        }
    }

    Ok(packets)
}

/// Packetize H.265 NAL units into RTP packets
///
/// Uses FU payloads for large NAL units per RFC 7798.
fn packetize_h265(data: &[u8], mut seq: u16, timestamp: u32, ssrc: u32) -> Result<Vec<Bytes>> {
    let mut packets = Vec::new();
    let nal_units = find_nal_units(data);
    let mut fragmenter = NalFragmenter::new(MAX_RTP_PAYLOAD_SIZE + 12);

    for (nal_index, nal) in nal_units.iter().enumerate() {
        let is_last_nal = nal_index + 1 == nal_units.len();
        let fragments = fragmenter.fragment_h265(nal);
        for fragment in fragments {
            let marker = if fragment.is_end && is_last_nal {
                0x80
            } else {
                0x00
            };
            let mut packet = Vec::with_capacity(12 + fragment.payload.len());
            packet.push(0x80);
            packet.push(marker | 96);
            packet.extend_from_slice(&seq.to_be_bytes());
            packet.extend_from_slice(&timestamp.to_be_bytes());
            packet.extend_from_slice(&ssrc.to_be_bytes());
            packet.extend_from_slice(&fragment.payload);
            packets.push(Bytes::from(packet));
            seq = seq.wrapping_add(1);
        }
    }

    Ok(packets)
}

/// Find NAL units in Annex B format data
fn find_nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    let mut i = 0;

    while i < data.len() {
        // Find start code
        let start = if data[i..].starts_with(&[0, 0, 0, 1]) {
            i + 4
        } else if data[i..].starts_with(&[0, 0, 1]) {
            i + 3
        } else {
            i += 1;
            continue;
        };

        if start >= data.len() {
            break;
        }

        // Find end (next start code or end of data)
        let mut end = data.len();
        for j in start..data.len().saturating_sub(2) {
            if data[j..].starts_with(&[0, 0, 0, 1]) || data[j..].starts_with(&[0, 0, 1]) {
                end = j;
                break;
            }
        }

        if start < end {
            units.push(&data[start..end]);
        }

        i = end;
    }

    // If no start codes found, treat entire buffer as one NAL unit
    if units.is_empty() && !data.is_empty() {
        units.push(data);
    }

    units
}

fn strip_annex_b(data: &[u8]) -> &[u8] {
    if data.starts_with(&[0, 0, 0, 1]) {
        &data[4..]
    } else if data.starts_with(&[0, 0, 1]) {
        &data[3..]
    } else {
        data
    }
}

fn h264_profile_level_id(sps: &[u8]) -> Option<String> {
    if sps.len() < 4 {
        return None;
    }
    Some(format!("{:02x}{:02x}{:02x}", sps[1], sps[2], sps[3]))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ice_server_stun() {
        let server = IceServer::stun("stun:stun.example.com:3478");
        assert_eq!(server.urls, vec!["stun:stun.example.com:3478"]);
        assert!(server.username.is_none());
        assert!(server.credential.is_none());
    }

    #[test]
    fn test_ice_server_turn() {
        let server = IceServer::turn("turn:turn.example.com:3478", "user", "pass");
        assert_eq!(server.urls, vec!["turn:turn.example.com:3478"]);
        assert_eq!(server.username, Some("user".to_string()));
        assert_eq!(server.credential, Some("pass".to_string()));
    }

    #[test]
    fn test_config_builder() {
        let config = WebRtcConfig::builder()
            .add_stun_server("stun:stun.l.google.com:19302")
            .add_turn_server("turn:turn.example.com:3478", "user", "pass")
            .ice_trickle(false)
            .ice_timeout_secs(60)
            .max_peers_per_camera(5)
            .video_codec(VideoCodecType::H264)
            .build()
            .unwrap();

        assert_eq!(config.ice_servers.len(), 2);
        assert!(!config.ice_trickle);
        assert_eq!(config.ice_connection_timeout_secs, 60);
        assert_eq!(config.max_peers_per_camera, 5);
        assert_eq!(config.video_codec, VideoCodecType::H264);
    }

    #[test]
    fn test_config_default_with_stun() {
        let config = WebRtcConfig::default_with_stun();
        assert!(!config.ice_servers.is_empty());
        assert!(config.ice_trickle);
    }

    #[test]
    fn test_sdp_offer_serialization() {
        let offer = SdpOffer {
            sdp_type: "offer".to_string(),
            sdp: "v=0\r\n...".to_string(),
        };

        let json = serde_json::to_string(&offer).unwrap();
        assert!(json.contains("\"type\":\"offer\""));

        let parsed: SdpOffer = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.sdp_type, "offer");
    }

    #[test]
    fn test_ice_candidate_serialization() {
        let candidate = IceCandidate {
            candidate: "candidate:1 1 UDP 2130706431 10.0.0.1 8080 typ host".to_string(),
            sdp_m_line_index: Some(0),
            sdp_mid: Some("video".to_string()),
            username_fragment: None,
        };

        let json = serde_json::to_string(&candidate).unwrap();
        assert!(json.contains("sdpMLineIndex"));
        assert!(json.contains("sdpMid"));
    }

    #[test]
    fn test_connection_state_from_rtc() {
        assert_eq!(
            ConnectionState::from(RTCPeerConnectionState::New),
            ConnectionState::Connecting
        );
        assert_eq!(
            ConnectionState::from(RTCPeerConnectionState::Connected),
            ConnectionState::Connected
        );
        assert_eq!(
            ConnectionState::from(RTCPeerConnectionState::Failed),
            ConnectionState::Failed
        );
    }

    #[test]
    fn test_find_nal_units_annex_b() {
        // Test with 4-byte start code
        let data = [0, 0, 0, 1, 0x67, 0x42, 0x00, 0, 0, 0, 1, 0x68, 0x00];
        let units = find_nal_units(&data);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0][0], 0x67); // SPS
        assert_eq!(units[1][0], 0x68); // PPS
    }

    #[test]
    fn test_find_nal_units_3byte_start_code() {
        // Test with 3-byte start code
        let data = [0, 0, 1, 0x65, 0x88, 0x84, 0, 0, 1, 0x61, 0x00];
        let units = find_nal_units(&data);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0][0], 0x65); // IDR
        assert_eq!(units[1][0], 0x61); // Non-IDR
    }

    #[test]
    fn test_find_nal_units_no_start_code() {
        // No start code - treat as single NAL
        let data = [0x65, 0x88, 0x84, 0x00];
        let units = find_nal_units(&data);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0], &data[..]);
    }

    #[test]
    fn test_packetize_h264_small_nal() {
        // Small NAL unit that fits in single packet
        let nal_data = vec![0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0xe9, 0x01];
        let packets = packetize_h264(&nal_data, 1000, 90000, 42).unwrap();

        assert!(!packets.is_empty());
        // First packet should contain the NAL unit
        assert!(packets[0].len() > 12); // At least RTP header
    }

    #[test]
    fn test_packetize_h264_large_nal() {
        // Large NAL unit that needs fragmentation
        let mut nal_data = vec![0, 0, 0, 1, 0x65]; // IDR slice
        nal_data.extend(vec![0x88; 3000]); // Large payload

        let packets = packetize_h264(&nal_data, 1000, 90000, 42).unwrap();

        // Should be fragmented into multiple FU-A packets
        assert!(packets.len() > 1);

        // First fragment should have start bit set
        assert_eq!(packets[0][12] & 0x1F, 28); // FU-A indicator
        assert!(packets[0][13] & 0x80 != 0); // Start bit

        // Last fragment should have end bit set
        let last = packets.last().unwrap();
        assert!(last[13] & 0x40 != 0); // End bit
    }

    #[test]
    fn test_signaling_message_serialization() {
        let offer = SignalingMessage::Offer(SdpOffer {
            sdp_type: "offer".to_string(),
            sdp: "v=0\r\n".to_string(),
        });

        let json = serde_json::to_string(&offer).unwrap();
        assert!(json.contains("\"type\":\"Offer\""));

        let answer = SignalingMessage::Answer(SdpAnswer {
            sdp_type: "answer".to_string(),
            sdp: "v=0\r\n".to_string(),
        });

        let json = serde_json::to_string(&answer).unwrap();
        assert!(json.contains("\"type\":\"Answer\""));
    }

    #[test]
    fn test_video_codec_type_default() {
        let codec: VideoCodecType = Default::default();
        assert_eq!(codec, VideoCodecType::H264);
    }

    #[test]
    fn test_packetize_h265_small_nal() {
        let nal_data = vec![0, 0, 0, 1, 0x40, 0x01, 0x0a, 0x0b];
        let packets = packetize_h265(&nal_data, 1000, 90000, 42).unwrap();
        assert!(!packets.is_empty());
    }

    #[tokio::test]
    async fn test_peer_connection_manager_creation() {
        let config = WebRtcConfig::default_with_stun();
        let manager = PeerConnectionManager::new(config).await;

        assert!(manager.is_ok());
        let manager = manager.unwrap();
        assert_eq!(manager.total_connection_count(), 0);
    }

    #[tokio::test]
    async fn test_whep_endpoint_creation() {
        let config = WebRtcConfig::default_with_stun();
        let manager = Arc::new(PeerConnectionManager::new(config).await.unwrap());
        let whep = WhepEndpoint::new(manager);

        assert!(whep.list_resources().is_empty());
    }

    #[test]
    fn test_whep_resource_fields() {
        let resource = WhepResource {
            resource_id: "test-id".to_string(),
            connection_id: "conn-123".to_string(),
            camera_id: "camera-456".to_string(),
            etag: "\"abc123\"".to_string(),
        };

        assert_eq!(resource.resource_id, "test-id");
        assert_eq!(resource.camera_id, "camera-456");
    }
}
