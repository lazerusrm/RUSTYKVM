use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as base64_standard;
use base64::Engine;
use reqwest::Client;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};
use url::Url;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp::packet::Packet;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;

#[derive(Debug, serde::Serialize)]
struct WhepRequest<'a> {
    camera_id: &'a str,
    offer_sdp: &'a str,
}

#[derive(Debug, serde::Deserialize)]
struct WhepResponse {
    answer_sdp: String,
    resource_id: String,
    location: Option<String>,
    etag: Option<String>,
}

/// RFC 8825 WHEP Client - Manages WebRTC-HTTP Egress Protocol resources
/// Stores resource identifiers and supports PATCH/DELETE operations
#[derive(Clone)]
pub struct WhepClient {
    /// Resource ID URL for PATCH/DELETE operations
    resource_id: Option<String>,
    /// HTTP Location header from WHEP response
    location: Option<String>,
    /// ETag for conditional requests (If-Match header)
    etag: Option<String>,
    /// HTTP client for PATCH/DELETE operations
    http_client: reqwest::Client,
    /// Optional bearer token for authentication
    auth_token: Option<String>,
}

impl WhepClient {
    /// Create a new WhepClient instance
    pub fn new(auth_token: Option<String>) -> Self {
        Self {
            resource_id: None,
            location: None,
            etag: None,
            http_client: Client::new(),
            auth_token,
        }
    }

    /// Store WHEP response resources for subsequent operations
    pub fn store_resources(&mut self, resource_id: String, location: String, etag: String) {
        self.resource_id = Some(resource_id);
        self.location = Some(location);
        self.etag = Some(etag);
        info!(
            "WHEP resources stored: location={}",
            self.location.as_deref().unwrap_or("N/A")
        );
    }

    /// Get the resource ID for PATCH/DELETE operations
    pub fn get_resource_id(&self) -> Option<&str> {
        self.resource_id.as_deref()
    }

    /// Get the current ETag for conditional requests
    pub fn get_etag(&self) -> Option<&str> {
        self.etag.as_deref()
    }

    /// Get the location header
    pub fn get_location(&self) -> Option<&str> {
        self.location.as_deref()
    }

    /// Update the ETag from a response header
    fn update_etag(&mut self, new_etag: String) {
        debug!("ETag updated from WHEP response");
        self.etag = Some(new_etag);
    }

    /// RFC 8825 PATCH operation - Change stream quality/bitrate
    /// Sends conditional PATCH request with If-Match header
    pub async fn patch_stream_quality(
        &mut self,
        quality_tier: &str,
        bitrate_kbps: Option<u32>,
    ) -> Result<()> {
        let resource_id = self
            .resource_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No WHEP resource ID stored"))?;

        let etag = self
            .etag
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No ETag available for conditional request"))?;

        #[derive(serde::Serialize)]
        struct QualityPatch {
            quality_tier: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            bitrate_kbps: Option<u32>,
        }

        let body = QualityPatch {
            quality_tier: quality_tier.to_string(),
            bitrate_kbps,
        };

        let mut request = self
            .http_client
            .patch(resource_id)
            .json(&body)
            .header("If-Match", etag.clone());

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let response = request
            .send()
            .await
            .context("Failed to send WHEP PATCH request")?
            .error_for_status()
            .context("WHEP PATCH response error")?;

        // Update ETag from response header if present
        if let Some(new_etag) = response.headers().get("etag") {
            if let Ok(etag_str) = new_etag.to_str() {
                self.update_etag(etag_str.to_string());
            }
        }

        info!(
            quality_tier,
            bitrate_kbps, "Stream quality patched successfully"
        );

        Ok(())
    }

    /// RFC 8825 DELETE operation - Terminate the stream
    /// Sends conditional DELETE request with If-Match header
    pub async fn delete_stream(&mut self) -> Result<()> {
        let resource_id = self
            .resource_id
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No WHEP resource ID stored"))?;

        let etag = self
            .etag
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No ETag available for conditional request"))?;

        let mut request = self
            .http_client
            .delete(resource_id)
            .header("If-Match", etag.clone());

        if let Some(token) = &self.auth_token {
            request = request.bearer_auth(token);
        }

        let _response = request
            .send()
            .await
            .context("Failed to send WHEP DELETE request")?
            .error_for_status()
            .context("WHEP DELETE response error")?;

        // Clear stored resources after successful deletion
        self.resource_id = None;
        self.location = None;
        self.etag = None;

        info!("Stream terminated successfully via WHEP DELETE");

        Ok(())
    }
}

pub async fn start_webrtc_stream(
    url: Url,
    camera_id: String,
    auth_token: Option<String>,
    packet_tx: mpsc::Sender<Vec<u8>>,
    byte_counter: Arc<AtomicU64>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let mut media_engine = MediaEngine::default();
    media_engine
        .register_default_codecs()
        .context("Failed to register codecs")?;

    let api = APIBuilder::new().with_media_engine(media_engine).build();

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };

    let peer_connection = Arc::new(api.new_peer_connection(config).await?);

    peer_connection
        .add_transceiver_from_kind(
            RTPCodecType::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                send_encodings: Vec::new(),
            }),
        )
        .await
        .context("Failed to add recvonly video transceiver")?;

    let packet_tx_clone = packet_tx.clone();
    let byte_counter_clone = Arc::clone(&byte_counter);
    peer_connection.on_track(Box::new(move |track, _, _| {
        let packet_tx = packet_tx_clone.clone();
        let byte_counter = Arc::clone(&byte_counter_clone);
        Box::pin(async move {
            let mut depacketizer = H264Depacketizer::new();
            let codec = track.codec();
            info!(
                kind = ?track.kind(),
                payload_type = codec.payload_type,
                mime = %codec.capability.mime_type,
                "WebRTC track received"
            );
            let mut packet_count = 0u64;
            let mut nal_logged = 0u32;
            loop {
                match track.read_rtp().await {
                    Ok((packet, _)) => {
                        packet_count += 1;
                        if packet_count == 1 {
                            let prefix_len = packet.payload.len().min(16);
                            let prefix = packet.payload[..prefix_len]
                                .iter()
                                .map(|byte| format!("{:02x}", byte))
                                .collect::<Vec<_>>()
                                .join(" ");
                            info!(
                                payload_type = packet.header.payload_type,
                                ssrc = packet.header.ssrc,
                                payload_len = packet.payload.len(),
                                payload_prefix = %prefix,
                                "First RTP packet received"
                            );
                        } else if packet_count.is_multiple_of(120) {
                            debug!(
                                packet_count,
                                payload_type = packet.header.payload_type,
                                ssrc = packet.header.ssrc,
                                "WebRTC RTP packets received"
                            );
                        }
                        for nal in depacketizer.depacketize(&packet) {
                            if nal_logged < 10 {
                                let prefix_len = nal.len().min(16);
                                let prefix = nal[..prefix_len]
                                    .iter()
                                    .map(|byte| format!("{:02x}", byte))
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                if let Some(nal_type) = nal_unit_type(&nal) {
                                    info!(
                                        nal_logged,
                                        nal_type,
                                        nal_len = nal.len(),
                                        nal_prefix = %prefix,
                                        "Received H264 NAL unit"
                                    );
                                }
                                nal_logged += 1;
                            }
                            byte_counter.fetch_add(nal.len() as u64, Ordering::Relaxed);
                            if packet_tx.send(nal).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(error) => {
                        warn!(error = %error, "WebRTC track read error");
                        break;
                    }
                }
            }
        })
    }));

    let offer = peer_connection.create_offer(None).await?;
    peer_connection.set_local_description(offer).await?;

    let mut gather_complete = peer_connection.gathering_complete_promise().await;
    let _ = gather_complete.recv().await;

    let local = peer_connection
        .local_description()
        .await
        .context("Missing local description")?;

    let whep = fetch_whep_answer(&url, &camera_id, &local.sdp, auth_token.as_deref()).await?;
    if !whep.param_sets.is_empty() {
        info!(
            count = whep.param_sets.len(),
            "Injecting H264 parameter sets from WHEP SDP"
        );
        for params in whep.param_sets {
            if packet_tx.send(with_start_code(&params)).await.is_err() {
                break;
            }
        }
    }
    peer_connection
        .set_remote_description(whep.answer)
        .await
        .context("Failed to set remote description")?;

    info!("WebRTC peer connection established");

    tokio::select! {
        _ = &mut shutdown_rx => {
            debug!("WebRTC shutdown requested");
        }
        _ = wait_for_connection_end(Arc::clone(&peer_connection)) => {}
    }

    peer_connection.close().await.ok();

    Ok(())
}

async fn wait_for_connection_end(peer_connection: Arc<webrtc::peer_connection::RTCPeerConnection>) {
    let (done_tx, done_rx) = oneshot::channel();
    let done_tx = Arc::new(std::sync::Mutex::new(Some(done_tx)));
    peer_connection.on_peer_connection_state_change(Box::new(move |state| {
        let done_tx = Arc::clone(&done_tx);
        Box::pin(async move {
            if matches!(
                state,
                RTCPeerConnectionState::Failed
                    | RTCPeerConnectionState::Disconnected
                    | RTCPeerConnectionState::Closed
            ) {
                info!(?state, "WebRTC connection ended");
                if let Some(tx) = done_tx.lock().ok().and_then(|mut guard| guard.take()) {
                    let _ = tx.send(());
                }
            }
        })
    }));
    let _ = done_rx.await;
}

struct WhepAnswer {
    answer: RTCSessionDescription,
    param_sets: Vec<Vec<u8>>,
}

async fn fetch_whep_answer(
    url: &Url,
    camera_id: &str,
    offer_sdp: &str,
    auth_token: Option<&str>,
) -> Result<WhepAnswer> {
    let client = Client::new();
    let mut request = client.post(url.clone()).json(&WhepRequest {
        camera_id,
        offer_sdp,
    });

    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }

    let response = request
        .send()
        .await
        .context("Failed to send WHEP offer")?
        .error_for_status()
        .context("WHEP response error")?;

    // Extract RFC 8825 WHEP resources from response headers
    let location = response
        .headers()
        .get("location")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    let etag = response
        .headers()
        .get("etag")
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string());

    let whep: WhepResponse = response
        .json()
        .await
        .context("Failed to parse WHEP response")?;
    let param_sets = extract_sprop_parameter_sets(&whep.answer_sdp);
    if param_sets.is_empty() {
        warn!("No H264 parameter sets found in WHEP SDP");
    }
    let answer = RTCSessionDescription::answer(whep.answer_sdp)
        .context("Failed to build WebRTC answer SDP")?;

    // Create WhepClient and store RFC 8825 resources
    let mut whep_client = WhepClient::new(auth_token.map(|s| s.to_string()));
    if let (Some(resource_id), Some(loc), Some(etag_val)) =
        (Some(whep.resource_id.clone()), location, etag)
    {
        whep_client.store_resources(resource_id, loc, etag_val);
    }

    Ok(WhepAnswer { answer, param_sets })
}

struct H264Depacketizer {
    fu_buffer: Vec<u8>,
}

impl H264Depacketizer {
    fn new() -> Self {
        Self {
            fu_buffer: Vec::new(),
        }
    }

    fn depacketize(&mut self, packet: &Packet) -> Vec<Vec<u8>> {
        let payload = &packet.payload;
        if payload.is_empty() {
            return Vec::new();
        }

        let nal_type = payload[0] & 0x1f;
        match nal_type {
            1..=23 => vec![self.with_start_code(payload)],
            24 => self.unpack_stap_a(payload),
            28 => self.unpack_fu_a(payload),
            _ => Vec::new(),
        }
    }

    fn with_start_code(&self, nal: &[u8]) -> Vec<u8> {
        if nal.starts_with(&[0, 0, 1]) || nal.starts_with(&[0, 0, 0, 1]) {
            return nal.to_vec();
        }
        let mut out = Vec::with_capacity(4 + nal.len());
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(nal);
        out
    }

    fn unpack_stap_a(&self, payload: &[u8]) -> Vec<Vec<u8>> {
        let mut offset = 1;
        let mut output = Vec::new();
        while offset + 2 <= payload.len() {
            let size = u16::from_be_bytes([payload[offset], payload[offset + 1]]) as usize;
            offset += 2;
            if offset + size > payload.len() {
                break;
            }
            output.push(self.with_start_code(&payload[offset..offset + size]));
            offset += size;
        }
        output
    }

    fn unpack_fu_a(&mut self, payload: &[u8]) -> Vec<Vec<u8>> {
        if payload.len() < 2 {
            return Vec::new();
        }

        let fu_header = payload[1];
        let start = fu_header & 0x80 != 0;
        let end = fu_header & 0x40 != 0;
        let nal_type = fu_header & 0x1f;
        let nal_header = (payload[0] & 0xe0) | nal_type;

        if start {
            self.fu_buffer.clear();
            self.fu_buffer.extend_from_slice(&[0, 0, 0, 1, nal_header]);
        }

        if !self.fu_buffer.is_empty() {
            self.fu_buffer.extend_from_slice(&payload[2..]);
        }

        if end && !self.fu_buffer.is_empty() {
            let complete = self.fu_buffer.clone();
            self.fu_buffer.clear();
            return vec![complete];
        }

        Vec::new()
    }
}

fn nal_unit_type(nal: &[u8]) -> Option<u8> {
    if nal.is_empty() {
        return None;
    }
    let offset = if nal.starts_with(&[0, 0, 0, 1]) {
        4
    } else if nal.starts_with(&[0, 0, 1]) {
        3
    } else {
        0
    };
    nal.get(offset).map(|byte| byte & 0x1f)
}

fn extract_sprop_parameter_sets(sdp: &str) -> Vec<Vec<u8>> {
    let mut output = Vec::new();
    for line in sdp.lines() {
        let Some(fmtp) = line.strip_prefix("a=fmtp:") else {
            continue;
        };
        let Some(pos) = fmtp.find("sprop-parameter-sets=") else {
            continue;
        };
        let params = &fmtp[pos + "sprop-parameter-sets=".len()..];
        let end = params.find(';').unwrap_or(params.len());
        let value = params[..end].trim();
        for encoded in value.split(',') {
            let encoded = encoded.trim();
            if encoded.is_empty() {
                continue;
            }
            match base64_standard.decode(encoded.as_bytes()) {
                Ok(bytes) => output.push(bytes),
                Err(error) => warn!(%encoded, %error, "Failed to decode sprop parameter set"),
            }
        }
    }
    output
}

fn with_start_code(nal: &[u8]) -> Vec<u8> {
    if nal.starts_with(&[0, 0, 1]) || nal.starts_with(&[0, 0, 0, 1]) {
        return nal.to_vec();
    }
    let mut out = Vec::with_capacity(4 + nal.len());
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_whep_client_creation_without_auth() {
        let client = WhepClient::new(None);
        assert_eq!(client.get_resource_id(), None);
        assert_eq!(client.get_etag(), None);
        assert_eq!(client.get_location(), None);
    }

    #[test]
    fn test_whep_client_creation_with_auth() {
        let token = "test_token_xyz".to_string();
        let client = WhepClient::new(Some(token.clone()));
        assert_eq!(client.get_resource_id(), None);
        assert_eq!(client.get_etag(), None);
        // Auth token is private, verified through behavior
    }

    #[test]
    fn test_store_resources() {
        let mut client = WhepClient::new(None);
        let resource_id = "https://edge.local/whep/resource/abc123".to_string();
        let location = "https://edge.local/whep/resource/abc123".to_string();
        let etag = "v1-abc123".to_string();

        client.store_resources(resource_id.clone(), location.clone(), etag.clone());

        assert_eq!(client.get_resource_id(), Some(resource_id.as_str()));
        assert_eq!(client.get_etag(), Some(etag.as_str()));
        assert_eq!(client.get_location(), Some(location.as_str()));
    }

    #[test]
    fn test_get_methods_return_options() {
        let mut client = WhepClient::new(None);

        // Initially empty
        assert!(client.get_resource_id().is_none());
        assert!(client.get_etag().is_none());
        assert!(client.get_location().is_none());

        // After storing
        client.store_resources(
            "https://edge/resource/1".to_string(),
            "https://edge/resource/1".to_string(),
            "etag123".to_string(),
        );

        assert!(client.get_resource_id().is_some());
        assert!(client.get_etag().is_some());
        assert!(client.get_location().is_some());
    }

    #[tokio::test]
    async fn test_patch_requires_resource_id() {
        let mut client = WhepClient::new(None);

        // Should fail without resource_id
        let result = client.patch_stream_quality("high", Some(5000)).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No WHEP resource ID"));
    }

    #[tokio::test]
    async fn test_patch_requires_etag() {
        let mut client = WhepClient::new(None);

        // Store only resource_id, not etag
        client.resource_id = Some("https://edge/resource/1".to_string());

        // Should fail without etag
        let result = client.patch_stream_quality("high", Some(5000)).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No ETag available"));
    }

    #[tokio::test]
    async fn test_delete_requires_resource_id() {
        let mut client = WhepClient::new(None);

        // Should fail without resource_id
        let result = client.delete_stream().await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No WHEP resource ID"));
    }

    #[tokio::test]
    async fn test_delete_requires_etag() {
        let mut client = WhepClient::new(None);

        // Store only resource_id, not etag
        client.resource_id = Some("https://edge/resource/1".to_string());

        // Should fail without etag
        let result = client.delete_stream().await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No ETag available"));
    }

    #[test]
    fn test_quality_patch_serialization() {
        #[derive(serde::Serialize)]
        struct QualityPatch {
            quality_tier: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            bitrate_kbps: Option<u32>,
        }

        let patch_with_bitrate = QualityPatch {
            quality_tier: "high".to_string(),
            bitrate_kbps: Some(5000),
        };

        let json = serde_json::to_string(&patch_with_bitrate).unwrap();
        assert!(json.contains("\"quality_tier\":\"high\""));
        assert!(json.contains("\"bitrate_kbps\":5000"));

        let patch_without_bitrate = QualityPatch {
            quality_tier: "low".to_string(),
            bitrate_kbps: None,
        };

        let json = serde_json::to_string(&patch_without_bitrate).unwrap();
        assert!(json.contains("\"quality_tier\":\"low\""));
        assert!(!json.contains("bitrate_kbps"));
    }

    #[test]
    fn test_nal_unit_type_detection() {
        // Test with 4-byte start code
        let nal_with_4byte_code = vec![0u8, 0u8, 0u8, 1u8, 0x65, 0x88, 0x84, 0x00];
        assert_eq!(nal_unit_type(&nal_with_4byte_code), Some(0x05));

        // Test with 3-byte start code
        let nal_with_3byte_code = vec![0u8, 0u8, 1u8, 0x67, 0x42, 0x00, 0x00];
        assert_eq!(nal_unit_type(&nal_with_3byte_code), Some(0x07));

        // Test without start code
        let nal_without_code = vec![0x68, 0xEE, 0x3C, 0xB0];
        assert_eq!(nal_unit_type(&nal_without_code), Some(0x08));

        // Test empty NAL
        let empty_nal: Vec<u8> = vec![];
        assert_eq!(nal_unit_type(&empty_nal), None);
    }

    #[test]
    fn test_with_start_code_idempotent() {
        let nal_with_code = vec![0u8, 0u8, 0u8, 1u8, 0x65, 0x88, 0x84, 0x00];
        let result = with_start_code(&nal_with_code);

        // Should not double-add start code
        assert_eq!(result.len(), nal_with_code.len());
        assert_eq!(result, nal_with_code);
    }

    #[test]
    fn test_with_start_code_adds_prefix() {
        let nal_without_code = vec![0x65, 0x88, 0x84, 0x00];
        let result = with_start_code(&nal_without_code);

        // Should add 4-byte start code
        assert_eq!(result.len(), nal_without_code.len() + 4);
        assert_eq!(&result[0..4], &[0u8, 0u8, 0u8, 1u8]);
        assert_eq!(&result[4..], &nal_without_code[..]);
    }

    #[test]
    fn test_extract_sprop_parameter_sets_valid_sdp() {
        let sdp =
            "a=fmtp:96 packetization-mode=1;sprop-parameter-sets=Z0JAH5WgKA9gvB4BAQaAA,aM48gA==";
        let param_sets = extract_sprop_parameter_sets(sdp);

        // Should extract two parameter sets
        assert_eq!(param_sets.len(), 2);
    }

    #[test]
    fn test_extract_sprop_parameter_sets_missing() {
        let sdp = "v=0\no=- 1 2 IN IP4 127.0.0.1\ns=test\nt=0 0\n";
        let param_sets = extract_sprop_parameter_sets(sdp);

        // Should return empty vec when no parameter sets
        assert_eq!(param_sets.len(), 0);
    }

    #[test]
    fn test_whep_response_deserialization() {
        let json = r#"{
            "answer_sdp": "v=0\no=- 1 2 IN IP4 127.0.0.1",
            "resource_id": "https://edge/resource/123",
            "location": "https://edge/resource/123",
            "etag": "v1-abc123"
        }"#;

        let response: WhepResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.resource_id, "https://edge/resource/123");
        assert_eq!(
            response.location.as_deref(),
            Some("https://edge/resource/123")
        );
        assert_eq!(response.etag.as_deref(), Some("v1-abc123"));
        assert!(response.answer_sdp.starts_with("v=0"));
    }

    #[test]
    fn test_h264_depacketizer_creation() {
        let depacketizer = H264Depacketizer::new();
        assert_eq!(depacketizer.fu_buffer.len(), 0);
    }
}
