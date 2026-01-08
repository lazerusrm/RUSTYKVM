//! WebRTC signaling for live view streaming.
//!
//! This module provides WebRTC signaling infrastructure for peer-to-peer
//! video streaming to browsers and clients.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info};

use crate::core::{ParameterSets, QualityTier};

/// ICE server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

/// WebRTC configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRtcConfig {
    pub ice_servers: Vec<IceServer>,
    pub ice_candidate_pool_size: u32,
    pub bundle_policy: String,
    pub rtcp_mux_policy: String,
}

impl Default for WebRtcConfig {
    fn default() -> Self {
        Self {
            ice_servers: vec![IceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                username: None,
                credential: None,
            }],
            ice_candidate_pool_size: 10,
            bundle_policy: "max-bundle".to_string(),
            rtcp_mux_policy: "require".to_string(),
        }
    }
}

/// Peer connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

/// Peer connection info
#[derive(Debug, Clone)]
pub struct PeerConnection {
    pub peer_id: String,
    pub camera_id: String,
    pub state: PeerState,
    pub created_at: Instant,
    pub last_activity: Instant,
    pub stats: PeerStats,
}

/// Peer statistics
#[derive(Debug, Clone, Default)]
pub struct PeerStats {
    pub bytes_received: u64,
    pub bytes_sent: u64,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub packets_lost: u64,
    pub jitter_ms: f64,
    pub rtt_ms: f64,
}

/// WebRTC signaling service
pub struct WebRtcSignaling {
    config: WebRtcConfig,
    peers: RwLock<HashMap<String, PeerConnection>>,
    events: broadcast::Sender<SignalingEvent>,
    metrics: Arc<WebRtcMetrics>,
    next_peer_id: AtomicU64,
}

impl WebRtcSignaling {
    pub fn new(config: WebRtcConfig) -> Self {
        let (events, _) = broadcast::channel(100);
        Self {
            config,
            peers: RwLock::new(HashMap::new()),
            events,
            metrics: Arc::new(WebRtcMetrics::default()),
            next_peer_id: AtomicU64::new(0),
        }
    }

    pub fn new_default() -> Self {
        Self::new(WebRtcConfig::default())
    }

    pub async fn create_offer(&self, camera_id: &str) -> Result<OfferResponse, SignalingError> {
        let peer_id = format!("peer-{}", self.next_peer_id.fetch_add(1, Ordering::Relaxed));

        let peer = PeerConnection {
            peer_id: peer_id.clone(),
            camera_id: camera_id.to_string(),
            state: PeerState::New,
            created_at: Instant::now(),
            last_activity: Instant::now(),
            stats: PeerStats::default(),
        };

        self.peers.write().await.insert(peer_id.clone(), peer);
        self.metrics.total_peers.fetch_add(1, Ordering::Relaxed);

        debug!(peer_id = %peer_id, camera_id = %camera_id, "Created offer");

        Ok(OfferResponse {
            peer_id,
            offer: SdpOffer {
                sdp: format!(
                    "v=0\n\
                    o=- 0 0 IN IP4 0.0.0.0\n\
                    s=NeuralMatrix\n\
                    t=0 0\n\
                    a=group:BUNDLE video\n\
                    m=video 9 UDP/TLS/RTP/SAVPF 96 97\n\
                    c=IN IP4 0.0.0.0\n\
                    a=mid:video\n\
                    a=rtcp-mux\n\
                    a=rtpmap:96 H264/90000\n\
                    a=rtpmap:97 VP8/90000"
                ),
                session_id: 0,
                version: 0,
            },
            config: self.config.clone(),
        })
    }

    pub async fn handle_answer(
        &self,
        peer_id: &str,
        _answer: SdpAnswer,
    ) -> Result<(), SignalingError> {
        let mut peers = self.peers.write().await;
        if let Some(peer) = peers.get_mut(peer_id) {
            peer.state = PeerState::Connecting;
            peer.last_activity = Instant::now();
            debug!(peer_id = %peer_id, "Answer received");
            Ok(())
        } else {
            Err(SignalingError::PeerNotFound)
        }
    }

    pub async fn add_ice_candidate(
        &self,
        peer_id: &str,
        candidate: IceCandidate,
    ) -> Result<(), SignalingError> {
        let mut peers = self.peers.write().await;
        if let Some(peer) = peers.get_mut(peer_id) {
            peer.last_activity = Instant::now();
            debug!(peer_id = %peer_id, candidate = %candidate.candidate, "ICE candidate added");
            Ok(())
        } else {
            Err(SignalingError::PeerNotFound)
        }
    }

    pub async fn request_quality_switch(
        &self,
        peer_id: &str,
        from_tier: QualityTier,
        to_tier: QualityTier,
    ) -> Result<(), SignalingError> {
        let peers = self.peers.read().await;
        let peer = peers.get(peer_id).ok_or(SignalingError::PeerNotFound)?;

        let request = QualitySwitchRequest {
            peer_id: peer_id.to_string(),
            camera_id: peer.camera_id.clone(),
            from_tier,
            to_tier,
        };

        let _ = self
            .events
            .send(SignalingEvent::QualitySwitchRequested { request });
        Ok(())
    }

    pub async fn complete_quality_switch(
        &self,
        response: QualitySwitchResponse,
    ) -> Result<(), SignalingError> {
        let peers = self.peers.read().await;
        if !peers.contains_key(&response.peer_id) {
            return Err(SignalingError::PeerNotFound);
        }

        let _ = self
            .events
            .send(SignalingEvent::QualitySwitchCompleted { response });
        Ok(())
    }

    pub async fn notify_gop_boundary(
        &self,
        notification: GopBoundaryNotification,
    ) -> Result<(), SignalingError> {
        let peers = self.peers.read().await;
        if !peers.contains_key(&notification.peer_id) {
            return Err(SignalingError::PeerNotFound);
        }

        let _ = self
            .events
            .send(SignalingEvent::GopBoundaryNotified { notification });
        Ok(())
    }

    pub async fn deliver_parameter_sets(
        &self,
        delivery: ParameterSetDelivery,
    ) -> Result<(), SignalingError> {
        let peers = self.peers.read().await;
        if !peers.contains_key(&delivery.peer_id) {
            return Err(SignalingError::PeerNotFound);
        }

        let _ = self
            .events
            .send(SignalingEvent::ParameterSetsDelivered { delivery });
        Ok(())
    }

    pub async fn handle_client_feedback(
        &self,
        feedback: ClientFeedback,
    ) -> Result<(), SignalingError> {
        let peers = self.peers.read().await;
        if !peers.contains_key(&feedback.peer_id) {
            return Err(SignalingError::PeerNotFound);
        }

        let _ = self
            .events
            .send(SignalingEvent::ClientFeedbackReceived { feedback });
        Ok(())
    }

    pub async fn close_peer(&self, peer_id: &str) -> Result<(), SignalingError> {
        let mut peers = self.peers.write().await;
        if let Some(_peer) = peers.remove(peer_id) {
            self.metrics.closed_peers.fetch_add(1, Ordering::Relaxed);
            info!(peer_id = %peer_id, "Peer connection closed");
            Ok(())
        } else {
            Err(SignalingError::PeerNotFound)
        }
    }

    pub async fn get_peer_state(&self, peer_id: &str) -> Option<PeerState> {
        self.peers.read().await.get(peer_id).map(|p| p.state)
    }

    pub async fn list_active_peers(&self) -> Vec<String> {
        self.peers.read().await.keys().cloned().collect()
    }

    pub async fn metrics(&self) -> WebRtcMetricsSnapshot {
        WebRtcMetricsSnapshot {
            total_peers: self.metrics.total_peers.load(Ordering::Relaxed),
            active_peers: self.peers.read().await.len() as u64,
            closed_peers: self.metrics.closed_peers.load(Ordering::Relaxed),
            failed_peers: self.metrics.failed_peers.load(Ordering::Relaxed),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SignalingEvent> {
        self.events.subscribe()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfferResponse {
    pub peer_id: String,
    pub offer: SdpOffer,
    pub config: WebRtcConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdpOffer {
    pub sdp: String,
    pub session_id: u64,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdpAnswer {
    pub sdp: String,
    pub session_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceCandidate {
    pub candidate: String,
    pub sdp_mid: String,
    pub sdp_mline_index: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignalingEvent {
    PeerConnected {
        peer_id: String,
        camera_id: String,
    },
    PeerDisconnected {
        peer_id: String,
    },
    PeerFailed {
        peer_id: String,
        reason: String,
    },
    IceCandidateReceived {
        peer_id: String,
    },
    QualitySwitchRequested {
        request: QualitySwitchRequest,
    },
    QualitySwitchCompleted {
        response: QualitySwitchResponse,
    },
    GopBoundaryNotified {
        notification: GopBoundaryNotification,
    },
    ParameterSetsDelivered {
        delivery: ParameterSetDelivery,
    },
    ClientFeedbackReceived {
        feedback: ClientFeedback,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SignalingError {
    #[error("Peer not found")]
    PeerNotFound,
    #[error("Invalid state")]
    InvalidState,
    #[error("Signaling error: {0}")]
    Signalling(String),
}

#[derive(Debug, Default)]
pub struct WebRtcMetrics {
    total_peers: AtomicU64,
    closed_peers: AtomicU64,
    failed_peers: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct WebRtcMetricsSnapshot {
    pub total_peers: u64,
    pub active_peers: u64,
    pub closed_peers: u64,
    pub failed_peers: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualitySwitchRequest {
    pub peer_id: String,
    pub camera_id: String,
    pub from_tier: QualityTier,
    pub to_tier: QualityTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualitySwitchResponse {
    pub peer_id: String,
    pub camera_id: String,
    pub accepted: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GopBoundaryNotification {
    pub peer_id: String,
    pub camera_id: String,
    pub tier: QualityTier,
    pub gop_number: u64,
    pub pts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterSetDelivery {
    pub peer_id: String,
    pub camera_id: String,
    pub tier: QualityTier,
    pub parameter_sets: ParameterSets,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientFeedback {
    pub peer_id: String,
    pub camera_id: String,
    pub estimated_bandwidth_kbps: Option<u64>,
    pub buffer_ms: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_offer() {
        let signaling = WebRtcSignaling::new_default();
        let response = signaling.create_offer("cam-001").await.unwrap();
        assert!(!response.peer_id.is_empty());
        assert!(!response.offer.sdp.is_empty());
    }

    #[tokio::test]
    async fn test_close_peer() {
        let signaling = WebRtcSignaling::new_default();
        let response = signaling.create_offer("cam-001").await.unwrap();
        signaling.close_peer(&response.peer_id).await.unwrap();
        assert!(signaling.get_peer_state(&response.peer_id).await.is_none());
    }

    #[tokio::test]
    async fn test_list_peers() {
        let signaling = WebRtcSignaling::new_default();
        signaling.create_offer("cam-001").await.unwrap();
        signaling.create_offer("cam-002").await.unwrap();

        let peers = signaling.list_active_peers().await;
        assert_eq!(peers.len(), 2);
    }
}
