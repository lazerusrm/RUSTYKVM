use crate::webrtc::transport::{IceCandidate, SdpOffer};
use crate::AppState;
use axum::{
    extract::{
        ws::{Message as WsMessage, WebSocket},
        State,
    },
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Debug, Serialize, Deserialize)]
pub struct SignalingMessage {
    pub event: String,
    pub data: String,
}

/// The RTCSessionDescription format sent by the browser
#[derive(Debug, Deserialize)]
struct RtcSessionDescription {
    #[serde(rename = "type")]
    sdp_type: String,
    sdp: String,
}

pub async fn h264_ws_handler(
    ws: axum::extract::WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws_signaling(socket, state))
}

async fn handle_ws_signaling(socket: WebSocket, state: Arc<AppState>) {
    info!("WebRTC Signaling WebSocket connected");

    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (msg_tx, mut msg_rx) = mpsc::channel::<SignalingMessage>(32);

    // Task to send messages to WebSocket
    let ws_sender_task = tokio::spawn(async move {
        while let Some(msg) = msg_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_sender.send(WsMessage::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    let mut peer_handle = None;
    let webrtc_mgr = state.webrtc.clone();

    // Main loop to receive from WebSocket
    while let Some(msg) = ws_receiver.next().await {
        let msg = match msg {
            Ok(msg) => msg,
            Err(e) => {
                warn!("WebRTC signaling websocket error: {}", e);
                break;
            }
        };

        if let WsMessage::Text(text) = msg {
            let sig_msg: SignalingMessage = match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    error!("Failed to parse signaling message: {}", e);
                    continue;
                }
            };

            match sig_msg.event.as_str() {
                "video-offer" => {
                    // Parse the JSON data to extract the SDP string
                    // Frontend sends: {"type":"offer","sdp":"v=0\r\n..."}
                    let sdp = match serde_json::from_str::<RtcSessionDescription>(&sig_msg.data) {
                        Ok(desc) => desc.sdp,
                        Err(e) => {
                            error!("Failed to parse SDP offer JSON: {}", e);
                            continue;
                        }
                    };

                    let offer = SdpOffer {
                        sdp_type: "offer".to_string(),
                        sdp,
                    };

                    // Make sure we have SPS/PPS before we build the local SDP.
                    // Our SDP includes `sprop-parameter-sets`, which the browser
                    // expects for H.264 decoding.
                    #[cfg(target_os = "linux")]
                    {
                        if !webrtc_mgr.has_h264_parameter_sets("default") {
                            let (width, height, bitrate) = {
                                let cfg = state.screen_config.read();
                                (cfg.width, cfg.height, cfg.bitrate)
                            };

                            let capture_state = state.clone();
                            match tokio::task::spawn_blocking(move || {
                                let start = std::time::Instant::now();
                                let mut last_err: Option<String> = None;

                                while start.elapsed() < Duration::from_secs(2) {
                                    match capture_state.kvm.get_h264(width, height, bitrate) {
                                        Ok(frame) => {
                                            let data = frame.into_bytes();
                                            let (sps, pps) = crate::webrtc::transport::extract_h264_parameter_sets(&data);
                                            if let (Some(sps), Some(pps)) = (sps, pps) {
                                                return Ok::<(bytes::Bytes, bytes::Bytes), String>((sps, pps));
                                            }
                                        }
                                        Err(e) => last_err = Some(e.to_string()),
                                    }
                                    std::thread::sleep(Duration::from_millis(30));
                                }

                                Err(last_err.unwrap_or_else(|| "timeout waiting for SPS/PPS".to_string()))
                            })
                            .await
                            {
                                Ok(Ok((sps, pps))) => {
                                    info!(
                                        "H.264 parameter sets primed for SDP negotiation: SPS {} bytes, PPS {} bytes",
                                        sps.len(),
                                        pps.len()
                                    );
                                    webrtc_mgr.update_h264_parameter_sets("default", sps, pps);
                                }
                                Ok(Err(e)) => {
                                    warn!("Unable to prime H.264 parameter sets (continuing): {}", e);
                                }
                                Err(e) => {
                                    warn!("H.264 parameter set prefetch task join error (continuing): {}", e);
                                }
                            }
                        }
                    }

                    match webrtc_mgr.handle_offer("default", offer).await {
                        Ok((answer, mut handle)) => {
                            // Send answer as JSON object matching RTCSessionDescription format
                            let answer_json = serde_json::json!({
                                "type": "answer",
                                "sdp": answer.sdp
                            });
                            let _ = msg_tx
                                .send(SignalingMessage {
                                    event: "video-answer".to_string(),
                                    data: answer_json.to_string(),
                                })
                                .await;

                            let msg_tx_clone = msg_tx.clone();
                            let conn_id = handle.connection_id.clone();
                            let webrtc_mgr_clone = webrtc_mgr.clone();
                            let peer_handle_clone = conn_id.clone();

                            // Spawn task to forward local candidates
                            tokio::spawn(async move {
                                while let Some(candidate) = handle.next_ice_candidate().await {
                                    let cand_msg = SignalingMessage {
                                        event: "video-candidate".to_string(),
                                        data: serde_json::to_string(&candidate).unwrap(),
                                    };
                                    if msg_tx_clone.send(cand_msg).await.is_err() {
                                        break;
                                    }
                                }
                                let _ = webrtc_mgr_clone.remove_connection(&conn_id).await;
                            });

                            peer_handle = Some(peer_handle_clone);
                        }
                        Err(e) => {
                            error!("Failed to handle WebRTC offer: {}", e);
                        }
                    }
                }
                "video-candidate" => {
                    if let Some(conn_id) = &peer_handle {
                        match serde_json::from_str::<IceCandidate>(&sig_msg.data) {
                            Ok(candidate) => {
                                if let Err(e) =
                                    webrtc_mgr.add_ice_candidate(conn_id, candidate).await
                                {
                                    error!("Failed to add ICE candidate: {}", e);
                                }
                            }
                            Err(e) => error!("Failed to parse ICE candidate: {}", e),
                        }
                    } else {
                        warn!("Received candidate before offer");
                    }
                }
                "heartbeat" => {
                    let _ = msg_tx
                        .send(SignalingMessage {
                            event: "heartbeat".to_string(),
                            data: "".to_string(),
                        })
                        .await;
                }
                _ => debug!("Unhandled signaling event: {}", sig_msg.event),
            }
        }
    }

    if let Some(conn_id) = peer_handle {
        let _ = webrtc_mgr.remove_connection(&conn_id).await;
    }

    ws_sender_task.abort();
    info!("WebRTC Signaling WebSocket disconnected");
}
