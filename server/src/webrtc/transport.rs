//! WebRTC transport layer for NanoKVM
//!
//! Provides WebRTC peer connections for low-latency HDMI streaming.
//! Optimized for single-source (HDMI) distribution to multiple clients.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use bytes::Bytes;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio::task::AbortHandle;
use uuid::Uuid;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_H264, MIME_TYPE_OPUS};
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

use crate::webrtc::{Error, FrameType, Result, VideoFrame};

// ============================================================================
// Shared Payload Optimization
// ============================================================================

/// A shared RTP payload fragment (NAL unit or FU-A fragment)
#[derive(Clone, Debug)]
pub struct SharedPacket {
    pub payload: Bytes,
    pub marker: bool,
}

impl SharedPacket {
    pub fn new(payload: Vec<u8>, marker: bool) -> Self {
        Self {
            payload: Bytes::from(payload),
            marker,
        }
    }
}

// ============================================================================
// Configuration Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl IceServer {
    pub fn stun(url: impl Into<String>) -> Self {
        Self {
            urls: vec![url.into()],
            username: None,
            credential: None,
        }
    }

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

    fn to_rtc_ice_server(&self) -> RTCIceServer {
        RTCIceServer {
            urls: self.urls.clone(),
            username: self.username.clone().unwrap_or_default(),
            credential: self.credential.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRtcConfig {
    pub ice_servers: Vec<IceServer>,
    #[serde(default = "default_ice_trickle")]
    pub ice_trickle: bool,
    #[serde(default = "default_ice_timeout")]
    pub ice_connection_timeout_secs: u64,
    #[serde(default = "default_max_peers")]
    pub max_peers_per_source: usize,
    #[serde(default = "default_video_codec")]
    pub video_codec: VideoCodecType,
    #[serde(default = "default_payload_type")]
    pub video_payload_type: u8,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VideoCodecType {
    #[default]
    H264,
    VP8,
}

impl WebRtcConfig {
    pub fn builder() -> WebRtcConfigBuilder {
        WebRtcConfigBuilder::default()
    }

    pub fn default_with_stun() -> Self {
        Self {
            ice_servers: vec![IceServer::stun("stun:stun.l.google.com:19302")],
            ice_trickle: true,
            ice_connection_timeout_secs: 30,
            max_peers_per_source: 10,
            video_codec: VideoCodecType::H264,
            video_payload_type: 96,
            video_clock_rate: 90000,
        }
    }

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

#[derive(Debug, Default)]
pub struct WebRtcConfigBuilder {
    ice_servers: Vec<IceServer>,
    ice_trickle: Option<bool>,
    ice_timeout: Option<u64>,
    max_peers: Option<usize>,
    video_codec: Option<VideoCodecType>,
}

impl WebRtcConfigBuilder {
    pub fn add_stun_server(mut self, url: impl Into<String>) -> Self {
        self.ice_servers.push(IceServer::stun(url));
        self
    }

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

    pub fn build(self) -> Result<WebRtcConfig> {
        Ok(WebRtcConfig {
            ice_servers: if self.ice_servers.is_empty() {
                vec![IceServer::stun("stun:stun.l.google.com:19302")]
            } else {
                self.ice_servers
            },
            ice_trickle: self.ice_trickle.unwrap_or(true),
            ice_connection_timeout_secs: self.ice_timeout.unwrap_or(30),
            max_peers_per_source: self.max_peers.unwrap_or(10),
            video_codec: self.video_codec.unwrap_or(VideoCodecType::H264),
            video_payload_type: 96,
            video_clock_rate: 90000,
        })
    }
}

// ============================================================================
// Signaling Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdpOffer {
    #[serde(rename = "type")]
    pub sdp_type: String,
    pub sdp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdpAnswer {
    #[serde(rename = "type")]
    pub sdp_type: String,
    pub sdp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceCandidate {
    pub candidate: String,
    #[serde(rename = "sdpMLineIndex")]
    pub sdp_m_line_index: Option<u16>,
    #[serde(rename = "sdpMid")]
    pub sdp_mid: Option<String>,
    #[serde(rename = "usernameFragment")]
    pub username_fragment: Option<String>,
}

impl IceCandidate {
    fn to_rtc_candidate_init(&self) -> RTCIceCandidateInit {
        RTCIceCandidateInit {
            candidate: self.candidate.clone(),
            sdp_mid: self.sdp_mid.clone(),
            sdp_mline_index: self.sdp_m_line_index,
            username_fragment: self.username_fragment.clone(),
        }
    }
}

// ============================================================================
// Peer Connection State
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Failed,
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

#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub connection_id: String,
    pub source_id: String,
    pub state: ConnectionState,
    pub ice_state: RTCIceConnectionState,
    pub created_at: std::time::Instant,
}

struct InternalConnection {
    connection_id: String,
    source_id: String,
    created_at: std::time::Instant,
    peer_connection: Arc<RTCPeerConnection>,
    video_track: Arc<TrackLocalStaticRTP>,
    audio_track: Arc<TrackLocalStaticRTP>,
    state_tx: broadcast::Sender<ConnectionState>,
    sequence_number: u16,
    audio_sequence_number: u16,
    ssrc: u32,
    audio_ssrc: u32,
    rtcp_abort_handle: Option<AbortHandle>,
    playout_delay_id: Option<u8>,
    client_caps: crate::webrtc::ClientCapabilities,
    video_codec: VideoCodecType,
}

pub struct PeerConnectionHandle {
    pub connection_id: String,
    pub source_id: String,
    peer_connection: Arc<RTCPeerConnection>,
    video_track: Arc<TrackLocalStaticRTP>,
    audio_track: Arc<TrackLocalStaticRTP>,
    state_rx: broadcast::Receiver<ConnectionState>,
    ice_candidate_rx: mpsc::UnboundedReceiver<IceCandidate>,
    sequence_number: u16,
    audio_sequence_number: u16,
    timestamp: u32,
    audio_timestamp: u32,
    ssrc: u32,
    audio_ssrc: u32,
    pub playout_delay_id: Option<u8>,
}

impl PeerConnectionHandle {
    pub fn state(&self) -> ConnectionState {
        self.peer_connection.connection_state().into()
    }
    pub fn is_active(&self) -> bool {
        matches!(
            self.state(),
            ConnectionState::Connected | ConnectionState::Connecting
        )
    }
    pub fn id(&self) -> &str {
        &self.connection_id
    }

    pub async fn send_video_frame(&mut self, frame: &VideoFrame) -> Result<()> {
        if !self.is_active() {
            return Err(Error::Transport("Connection not active".into()));
        }
        let packets = match frame.frame_type {
            FrameType::H264 => packetize_h264_optimized(&frame.data),
            _ => {
                return Err(Error::Transport(format!(
                    "Unsupported codec: {:?}",
                    frame.frame_type
                )))
            }
        };
        for shared in packets {
            let mut header = webrtc::rtp::header::Header {
                version: 2,
                marker: shared.marker,
                payload_type: 96,
                sequence_number: self.sequence_number,
                timestamp: self.timestamp,
                ssrc: self.ssrc,
                ..Default::default()
            };
            if let Some(id) = self.playout_delay_id {
                header
                    .set_extension(id, Bytes::from_static(&[0, 0, 0]))
                    .ok();
            }
            let packet = RtpPacket {
                header,
                payload: shared.payload,
            };
            self.video_track
                .write_rtp(&packet)
                .await
                .map_err(|e| Error::Transport(e.to_string()))?;
            self.sequence_number = self.sequence_number.wrapping_add(1);
        }
        self.timestamp = self.timestamp.wrapping_add(3000);
        Ok(())
    }

    pub async fn add_ice_candidate(&self, candidate: IceCandidate) -> Result<()> {
        self.peer_connection
            .add_ice_candidate(candidate.to_rtc_candidate_init())
            .await
            .map_err(|e| Error::Transport(e.to_string()))
    }

    pub async fn next_ice_candidate(&mut self) -> Option<IceCandidate> {
        self.ice_candidate_rx.next().await
    }
}

pub struct PeerConnectionManager {
    config: WebRtcConfig,
    api: webrtc::api::API,
    connections: Arc<RwLock<HashMap<String, InternalConnection>>>,
    source_connections: Arc<RwLock<HashMap<String, Vec<String>>>>,
    event_tx: broadcast::Sender<PeerEvent>,
    h264_parameter_sets: Arc<RwLock<HashMap<String, H264ParameterSets>>>,
}

#[derive(Debug, Clone, Default)]
pub struct H264ParameterSets {
    pub sps: Bytes,
    pub pps: Bytes,
}

#[derive(Debug, Clone)]
pub enum PeerEvent {
    Connected {
        connection_id: String,
        source_id: String,
    },
    Disconnected {
        connection_id: String,
        source_id: String,
    },
    IceCandidate {
        connection_id: String,
        candidate: IceCandidate,
    },
}

impl PeerConnectionManager {
    pub async fn new(config: WebRtcConfig) -> Result<Self> {
        let mut media_engine = MediaEngine::default();
        media_engine
            .register_default_codecs()
            .map_err(|e| Error::Config(e.to_string()))?;
        // Note: Opus is included in register_default_codecs() so no need to register separately

        // Note: Header extension registration not available in this webrtc version
        // playout-delay extension would be registered here if supported
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut media_engine)
            .map_err(|e| Error::Config(e.to_string()))?;
        let api = APIBuilder::new()
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_setting_engine(SettingEngine::default())
            .build();
        let (event_tx, _) = broadcast::channel(256);
        Ok(Self {
            config,
            api,
            connections: Arc::new(RwLock::new(HashMap::new())),
            source_connections: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            h264_parameter_sets: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub fn update_h264_parameter_sets(&self, source_id: &str, sps: Bytes, pps: Bytes) {
        if !sps.is_empty() && !pps.is_empty() {
            self.h264_parameter_sets
                .write()
                .insert(source_id.to_string(), H264ParameterSets { sps, pps });
        }
    }

    pub async fn handle_offer(
        &self,
        source_id: &str,
        offer: SdpOffer,
    ) -> Result<(SdpAnswer, PeerConnectionHandle)> {
        let current_count = self.connection_count_for_source(source_id);
        if current_count >= self.config.max_peers_per_source {
            return Err(Error::Transport(format!(
                "Max peers reached for source {}",
                source_id
            )));
        }
        let peer_connection = Arc::new(
            self.api
                .new_peer_connection(self.config.to_rtc_configuration())
                .await
                .map_err(|e| Error::Transport(e.to_string()))?,
        );
        let connection_id = Uuid::new_v4().to_string();
        let fmtp_line = self.h264_fmtp_line(source_id).unwrap_or_default();
        let video_track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_string(),
                clock_rate: self.config.video_clock_rate,
                channels: 0,
                sdp_fmtp_line: fmtp_line,
                rtcp_feedback: vec![],
            },
            format!("video-{}", connection_id),
            format!("stream-{}", source_id),
        ));
        let audio_track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_string(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                rtcp_feedback: vec![],
            },
            format!("audio-{}", connection_id),
            format!("audio-{}", source_id),
        ));

        let rtp_sender = peer_connection
            .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let _audio_sender = peer_connection
            .add_track(Arc::clone(&audio_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let rtp_sender_clone = rtp_sender.clone();
        let rtcp_task = tokio::spawn(async move {
            let mut rtcp_buf = vec![0u8; 1500];
            while let Ok((_, _)) = rtp_sender_clone.read(&mut rtcp_buf).await {}
        });
        let (state_tx, state_rx) = broadcast::channel(16);
        let (ice_tx, ice_rx) = mpsc::unbounded();
        let conn_id_clone = connection_id.clone();
        let event_tx_clone = self.event_tx.clone();
        peer_connection.on_ice_candidate(Box::new(move |candidate| {
            let conn_id = conn_id_clone.clone();
            let mut ice_tx = ice_tx.clone();
            let event_tx = event_tx_clone.clone();
            Box::pin(async move {
                if let Some(c) = candidate {
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
        let conn_id_clone = connection_id.clone();
        let src_id_clone = source_id.to_string();
        let event_tx_clone = self.event_tx.clone();
        let state_tx_clone = state_tx.clone();
        peer_connection.on_peer_connection_state_change(Box::new(move |state| {
            let conn_id = conn_id_clone.clone();
            let src_id = src_id_clone.clone();
            let event_tx = event_tx_clone.clone();
            let state_tx = state_tx_clone.clone();
            Box::pin(async move {
                let conn_state = ConnectionState::from(state);
                let _ = state_tx.send(conn_state);
                match conn_state {
                    ConnectionState::Connected => {
                        let _ = event_tx.send(PeerEvent::Connected {
                            connection_id: conn_id,
                            source_id: src_id,
                        });
                    }
                    ConnectionState::Disconnected
                    | ConnectionState::Failed
                    | ConnectionState::Closed => {
                        let _ = event_tx.send(PeerEvent::Disconnected {
                            connection_id: conn_id,
                            source_id: src_id,
                        });
                    }
                    _ => {}
                }
            })
        }));
        peer_connection
            .set_remote_description(
                RTCSessionDescription::offer(offer.sdp)
                    .map_err(|e| Error::Transport(e.to_string()))?,
            )
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let answer = peer_connection
            .create_answer(None)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        peer_connection
            .set_local_description(answer.clone())
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let receivers = peer_connection.get_receivers().await;
        let mut playout_delay_id: Option<u8> = None;
        if let Some(receiver) = receivers.first() {
            let params = receiver.get_parameters().await;
            playout_delay_id = params
                .header_extensions
                .iter()
                .find(|e| e.uri == "http://www.webrtc.org/experiments/rtp-hdrext/playout-delay")
                .map(|e| e.id as u8);
        }
        let local_desc = peer_connection
            .local_description()
            .await
            .ok_or_else(|| Error::Transport("No local description".into()))?;
        let sdp_answer = SdpAnswer {
            sdp_type: "answer".to_string(),
            sdp: local_desc.sdp.clone(),
        };
        let internal_conn = InternalConnection {
            connection_id: connection_id.clone(),
            source_id: source_id.to_string(),
            created_at: std::time::Instant::now(),
            peer_connection: Arc::clone(&peer_connection),
            video_track: video_track.clone(),
            audio_track: audio_track.clone(),
            state_tx,
            sequence_number: 0,
            audio_sequence_number: 0,
            ssrc: rand::random::<u32>(),
            audio_ssrc: rand::random::<u32>(),
            rtcp_abort_handle: Some(rtcp_task.abort_handle()),
            playout_delay_id,
            client_caps: crate::webrtc::ClientCapabilities::from_sdp(&local_desc.sdp),
            video_codec: VideoCodecType::H264,
        };
        self.connections
            .write()
            .insert(connection_id.clone(), internal_conn);
        self.source_connections
            .write()
            .entry(source_id.to_string())
            .or_default()
            .push(connection_id.clone());
        Ok((
            sdp_answer,
            PeerConnectionHandle {
                connection_id: connection_id.clone(),
                source_id: source_id.to_string(),
                peer_connection,
                video_track,
                audio_track,
                state_rx,
                ice_candidate_rx: ice_rx,
                sequence_number: 0,
                audio_sequence_number: 0,
                timestamp: 0,
                audio_timestamp: 0,
                ssrc: rand::random::<u32>(),
                audio_ssrc: rand::random::<u32>(),
                playout_delay_id,
            },
        ))
    }

    fn h264_fmtp_line(&self, source_id: &str) -> Option<String> {
        let sets = self.h264_parameter_sets.read().get(source_id).cloned()?;
        let sps = strip_annex_b(&sets.sps);
        let pps = strip_annex_b(&sets.pps);
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

    pub fn connection_count_for_source(&self, source_id: &str) -> usize {
        self.source_connections
            .read()
            .get(source_id)
            .map(|v| v.len())
            .unwrap_or(0)
    }
    pub fn total_connection_count(&self) -> usize {
        self.connections.read().len()
    }
    pub fn get_source_connections(&self, source_id: &str) -> Vec<String> {
        self.source_connections
            .read()
            .get(source_id)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn remove_connection(&self, connection_id: &str) -> Result<()> {
        let handle_opt = {
            let mut guard = self.connections.write();
            guard.remove(connection_id)
        };
        if let Some(handle) = handle_opt {
            if let Some(abort) = handle.rtcp_abort_handle {
                abort.abort();
            }
            {
                let mut guard = self.source_connections.write();
                if let Some(conns) = guard.get_mut(&handle.source_id) {
                    conns.retain(|id| id != connection_id);
                }
            }
            let _ = handle.peer_connection.close().await;
        }
        Ok(())
    }

    pub async fn broadcast_frame(
        &self,
        connection_ids: Vec<String>,
        timestamp: u32,
        packets: &[SharedPacket],
    ) -> Result<usize> {
        if connection_ids.is_empty() {
            return Ok(0);
        }

        let mut sent_count = 0;
        for conn_id in connection_ids {
            if self
                .send_optimized_packets(&conn_id, timestamp, packets)
                .await?
            {
                sent_count += 1;
            }
        }
        Ok(sent_count)
    }

    async fn send_optimized_packets(
        &self,
        connection_id: &str,
        timestamp: u32,
        packets: &[SharedPacket],
    ) -> Result<bool> {
        let (track, mut sequence_number, ssrc, playout_delay_id) = {
            let mut connections = self.connections.write();
            let conn = match connections.get_mut(connection_id) {
                Some(c) => c,
                None => return Ok(false),
            };
            (
                Arc::clone(&conn.video_track),
                conn.sequence_number,
                conn.ssrc,
                conn.playout_delay_id,
            )
        };
        for shared in packets {
            let mut header = webrtc::rtp::header::Header {
                version: 2,
                marker: shared.marker,
                payload_type: 96,
                sequence_number,
                timestamp,
                ssrc,
                ..Default::default()
            };
            if let Some(id) = playout_delay_id {
                header
                    .set_extension(id, Bytes::from_static(&[0, 0, 0]))
                    .ok();
            }
            let packet = RtpPacket {
                header,
                payload: shared.payload.clone(),
            };
            if track.write_rtp(&packet).await.is_ok() {
                sequence_number = sequence_number.wrapping_add(1);
            }
        }
        if let Some(conn) = self.connections.write().get_mut(connection_id) {
            conn.sequence_number = sequence_number;
        }
        Ok(true)
    }

    pub async fn broadcast_audio(
        &self,
        connection_ids: Vec<String>,
        timestamp: u32,
        payload: &Bytes,
    ) -> Result<usize> {
        if connection_ids.is_empty() {
            return Ok(0);
        }
        let mut sent_count = 0;
        for conn_id in connection_ids {
            if self.send_audio_packet(&conn_id, timestamp, payload).await? {
                sent_count += 1;
            }
        }
        Ok(sent_count)
    }

    async fn send_audio_packet(
        &self,
        connection_id: &str,
        timestamp: u32,
        payload: &Bytes,
    ) -> Result<bool> {
        let (track, mut sequence_number, ssrc) = {
            let mut connections = self.connections.write();
            let conn = match connections.get_mut(connection_id) {
                Some(c) => c,
                None => return Ok(false),
            };
            (
                Arc::clone(&conn.audio_track),
                conn.audio_sequence_number,
                conn.audio_ssrc,
            )
        };
        let header = webrtc::rtp::header::Header {
            version: 2,
            marker: true,
            payload_type: 111, // Opus payload type
            sequence_number,
            timestamp,
            ssrc,
            ..Default::default()
        };
        let packet = RtpPacket {
            header,
            payload: payload.clone(),
        };
        if track.write_rtp(&packet).await.is_ok() {
            sequence_number = sequence_number.wrapping_add(1);
            if let Some(conn) = self.connections.write().get_mut(connection_id) {
                conn.audio_sequence_number = sequence_number;
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn add_ice_candidate(
        &self,
        connection_id: &str,
        candidate: IceCandidate,
    ) -> Result<()> {
        let peer_connection = {
            let guard = self.connections.read();
            guard
                .get(connection_id)
                .map(|conn| Arc::clone(&conn.peer_connection))
        };
        if let Some(pc) = peer_connection {
            pc.add_ice_candidate(candidate.to_rtc_candidate_init())
                .await
                .map_err(|e| Error::Transport(e.to_string()))?;
        }
        Ok(())
    }

    pub async fn create_subscriber(
        &self,
        source_id: &str,
        offer: RTCSessionDescription,
    ) -> Result<(String, RTCSessionDescription)> {
        let current_count = self.connection_count_for_source(source_id);
        if current_count >= self.config.max_peers_per_source {
            return Err(Error::Transport(format!(
                "Max peers reached for source {}",
                source_id
            )));
        }
        let peer_connection = Arc::new(
            self.api
                .new_peer_connection(self.config.to_rtc_configuration())
                .await
                .map_err(|e| Error::Transport(e.to_string()))?,
        );
        let connection_id = Uuid::new_v4().to_string();
        let fmtp_line = self.h264_fmtp_line(source_id).unwrap_or_default();
        let video_track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.to_string(),
                clock_rate: self.config.video_clock_rate,
                channels: 0,
                sdp_fmtp_line: fmtp_line,
                rtcp_feedback: vec![],
            },
            format!("video-{}", connection_id),
            format!("stream-{}", source_id),
        ));
        let audio_track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_string(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_string(),
                rtcp_feedback: vec![],
            },
            format!("audio-{}", connection_id),
            format!("audio-{}", source_id),
        ));

        let _video_sender = peer_connection
            .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let _audio_sender = peer_connection
            .add_track(Arc::clone(&audio_track) as Arc<dyn TrackLocal + Send + Sync>)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let (state_tx, _state_rx) = broadcast::channel(16);
        let (ice_tx, _ice_rx) = mpsc::unbounded();
        let conn_id_clone = connection_id.clone();
        let event_tx_clone = self.event_tx.clone();
        peer_connection.on_ice_candidate(Box::new(move |candidate| {
            let conn_id = conn_id_clone.clone();
            let mut ice_tx = ice_tx.clone();
            let event_tx = event_tx_clone.clone();
            Box::pin(async move {
                if let Some(c) = candidate {
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
        let conn_id_clone = connection_id.clone();
        let src_id_clone = source_id.to_string();
        let event_tx_clone = self.event_tx.clone();
        let state_tx_clone = state_tx.clone();
        peer_connection.on_peer_connection_state_change(Box::new(move |state| {
            let conn_id = conn_id_clone.clone();
            let src_id = src_id_clone.clone();
            let event_tx = event_tx_clone.clone();
            let state_tx = state_tx_clone.clone();
            Box::pin(async move {
                let conn_state = ConnectionState::from(state);
                let _ = state_tx.send(conn_state);
                match conn_state {
                    ConnectionState::Connected => {
                        let _ = event_tx.send(PeerEvent::Connected {
                            connection_id: conn_id,
                            source_id: src_id,
                        });
                    }
                    ConnectionState::Disconnected
                    | ConnectionState::Failed
                    | ConnectionState::Closed => {
                        let _ = event_tx.send(PeerEvent::Disconnected {
                            connection_id: conn_id,
                            source_id: src_id,
                        });
                    }
                    _ => {}
                }
            })
        }));
        peer_connection
            .set_remote_description(offer)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let answer = peer_connection
            .create_answer(None)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        peer_connection
            .set_local_description(answer.clone())
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let local_desc = peer_connection
            .local_description()
            .await
            .ok_or_else(|| Error::Transport("No local description".into()))?;
        let internal_conn = InternalConnection {
            connection_id: connection_id.clone(),
            source_id: source_id.to_string(),
            created_at: std::time::Instant::now(),
            peer_connection: Arc::clone(&peer_connection),
            video_track: video_track.clone(),
            audio_track: audio_track.clone(),
            state_tx,
            sequence_number: 0,
            audio_sequence_number: 0,
            ssrc: rand::random::<u32>(),
            audio_ssrc: rand::random::<u32>(),
            rtcp_abort_handle: None,
            playout_delay_id: None,
            client_caps: crate::webrtc::ClientCapabilities::from_sdp(&local_desc.sdp),
            video_codec: VideoCodecType::H264,
        };
        self.connections
            .write()
            .insert(connection_id.clone(), internal_conn);
        self.source_connections
            .write()
            .entry(source_id.to_string())
            .or_default()
            .push(connection_id.clone());
        Ok((connection_id, local_desc))
    }

    pub async fn close_connection(&self, connection_id: &str) {
        let _ = self.remove_connection(connection_id).await;
    }
}

const MAX_RTP_PAYLOAD_SIZE: usize = 1200;

pub fn packetize_h264_optimized(data: &[u8]) -> Vec<SharedPacket> {
    let mut packets = Vec::new();
    let nal_units = find_nal_units(data);
    for (nal_index, nal) in nal_units.iter().enumerate() {
        let is_last_nal = nal_index + 1 == nal_units.len();
        if nal.len() <= MAX_RTP_PAYLOAD_SIZE {
            packets.push(SharedPacket::new(nal.to_vec(), is_last_nal));
        } else {
            let nal_header = nal[0];
            let (nal_type, nri) = (nal_header & 0x1F, nal_header & 0x60);
            let payload = &nal[1..];
            let mut offset = 0;
            let mut first = true;
            while offset < payload.len() {
                let chunk_size = std::cmp::min(payload.len() - offset, MAX_RTP_PAYLOAD_SIZE - 2);
                let last = offset + chunk_size >= payload.len();
                let mut fragment = Vec::with_capacity(2 + chunk_size);
                fragment.push(nri | 28);
                fragment.push(if first {
                    0x80 | nal_type
                } else if last {
                    0x40 | nal_type
                } else {
                    nal_type
                });
                fragment.extend_from_slice(&payload[offset..offset + chunk_size]);
                packets.push(SharedPacket::new(fragment, last && is_last_nal));
                offset += chunk_size;
                first = false;
            }
        }
    }
    packets
}

fn find_nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut units = Vec::new();
    let mut i = 0;

    // Use memchr to find 0x01, then check if it's part of 00 00 01 or 00 00 00 01
    while let Some(pos) = memchr::memchr(0x01, &data[i..]) {
        let abs_pos = i + pos;
        if abs_pos >= 2 && data[abs_pos - 1] == 0x00 && data[abs_pos - 2] == 0x00 {
            let start = abs_pos + 1;
            // Find next start code
            let mut end = data.len();
            let search_start = start;
            if let Some(next_pos) = memchr::memchr(0x01, &data[search_start..]) {
                let abs_next = search_start + next_pos;
                if abs_next >= 2 && data[abs_next - 1] == 0x00 && data[abs_next - 2] == 0x00 {
                    // Backtrack to the start of the next 00 00 01 or 00 00 00 01
                    end = abs_next - 2;
                    if end > 0 && data[end - 1] == 0x00 {
                        end -= 1;
                    }
                }
            }
            if start < end {
                units.push(&data[start..end]);
            }
            i = end;
        } else {
            i = abs_pos + 1;
        }
    }

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
// WHEP Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhepRequest {
    pub source_id: String,
    pub offer_sdp: String,
}

#[derive(Debug, Clone)]
pub struct WhepResponse {
    pub location: String,
    pub etag: String,
    pub answer_sdp: String,
}

pub struct WhepEndpoint {
    manager: Arc<PeerConnectionManager>,
    resources: RwLock<HashMap<String, String>>, // resource_id -> connection_id
}

impl WhepEndpoint {
    pub fn new(manager: Arc<PeerConnectionManager>) -> Self {
        Self {
            manager,
            resources: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create_resource(&self, req: WhepRequest) -> Result<WhepResponse> {
        let offer = RTCSessionDescription::offer(req.offer_sdp)
            .map_err(|e| Error::Config(e.to_string()))?;
        let (connection_id, answer) = self
            .manager
            .create_subscriber(&req.source_id, offer)
            .await?;

        let resource_id = Uuid::new_v4().to_string();
        {
            let mut resources = self.resources.write();
            resources.insert(resource_id.clone(), connection_id.clone());
        }

        Ok(WhepResponse {
            location: format!("/api/whep/{}", resource_id),
            etag: connection_id,
            answer_sdp: answer.sdp,
        })
    }

    pub fn get_resource(&self, resource_id: &str) -> Option<String> {
        let resources = self.resources.read();
        resources.get(resource_id).cloned()
    }

    pub async fn add_ice_candidate(
        &self,
        resource_id: &str,
        candidate: IceCandidate,
    ) -> Result<()> {
        let connection_id = {
            let resources = self.resources.read();
            resources.get(resource_id).cloned()
        };

        if let Some(conn_id) = connection_id {
            self.manager.add_ice_candidate(&conn_id, candidate).await
        } else {
            Err(Error::Transport("Resource not found".into()))
        }
    }

    pub async fn delete_resource(&self, resource_id: &str) -> Result<()> {
        let connection_id = {
            let mut resources = self.resources.write();
            resources.remove(resource_id)
        };

        if let Some(conn_id) = connection_id {
            self.manager.close_connection(&conn_id).await;
            Ok(())
        } else {
            Err(Error::Transport("Resource not found".into()))
        }
    }
}
