use crate::api::ApiResponse;
use axum::{extract::Json, response::IntoResponse};
use network::NetworkManager;
use serde::{Deserialize, Serialize};
use tracing::error;

#[derive(Deserialize)]
pub struct WakeOnLANReq {
    pub mac: String,
}

#[derive(Serialize)]
pub struct GetMacRsp {
    pub macs: Vec<String>,
}

#[derive(Deserialize)]
pub struct SetMacNameReq {
    pub mac: String,
    pub name: String,
}

#[derive(Deserialize)]
pub struct DeleteMacReq {
    pub mac: String,
}

#[derive(Serialize)]
pub struct GetWifiRsp {
    pub supported: bool,
    pub connected: bool,
    pub ssid: String,
    #[serde(rename = "apMode")]
    pub ap_mode: bool,
}

#[derive(Deserialize)]
pub struct ConnectWifiReq {
    pub ssid: String,
    pub password: String,
}

// --- WoL Handlers ---

pub async fn wol_handler(Json(req): Json<WakeOnLANReq>) -> impl IntoResponse {
    match NetworkManager::wake_on_lan(&req.mac).await {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => match e {
            network::NetworkError::InvalidMac(_) => Json(ApiResponse::<serde_json::Value>::err(
                -2,
                "invalid MAC address",
            ))
            .into_response(),
            network::NetworkError::CommandFailed(msg) => {
                error!("WoL command failed: {}", msg);
                Json(ApiResponse::<serde_json::Value>::err(-3, &msg)).into_response()
            }
            _ => {
                error!("WoL failed: {}", e);
                Json(ApiResponse::<serde_json::Value>::err(-1, &e.to_string())).into_response()
            }
        },
    }
}

pub async fn get_wol_macs_handler() -> impl IntoResponse {
    match NetworkManager::get_wol_macs().await {
        Ok(entries) => {
            let macs = entries
                .into_iter()
                .map(|e| {
                    if e.name.is_empty() {
                        e.mac
                    } else {
                        format!("{} {}", e.mac, e.name)
                    }
                })
                .collect();
            Json(ApiResponse::ok(GetMacRsp { macs })).into_response()
        }
        Err(e) => {
            error!("Failed to get WoL MACs: {}", e);
            Json(ApiResponse::<GetMacRsp>::err(-1, &e.to_string())).into_response()
        }
    }
}

pub async fn set_wol_name_handler(Json(req): Json<SetMacNameReq>) -> impl IntoResponse {
    match NetworkManager::set_wol_mac_name(&req.mac, &req.name).await {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => match e {
            network::NetworkError::InvalidMac(msg) => {
                // Go uses a distinct code when the MAC isn't present in the cache.
                Json(ApiResponse::<serde_json::Value>::err(-3, &msg)).into_response()
            }
            _ => {
                error!("Failed to set WoL name: {}", e);
                Json(ApiResponse::<serde_json::Value>::err(-1, &e.to_string())).into_response()
            }
        },
    }
}

pub async fn delete_wol_mac_handler(Json(req): Json<DeleteMacReq>) -> impl IntoResponse {
    match NetworkManager::delete_wol_mac(&req.mac).await {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => {
            error!("Failed to delete WoL MAC: {}", e);
            Json(ApiResponse::<serde_json::Value>::err(-1, &e.to_string())).into_response()
        }
    }
}

// --- WiFi Handlers ---

pub async fn get_wifi_handler() -> impl IntoResponse {
    let supported = NetworkManager::is_wifi_supported().await;
    let mut connected = false;
    let mut ssid = String::new();
    let mut ap_mode = false;

    if supported {
        connected = NetworkManager::is_wifi_connected().await;
        ap_mode = NetworkManager::is_wifi_ap_mode().await;
        if connected {
            ssid = NetworkManager::get_wifi_ssid()
                .await
                .unwrap_or_else(|| "Wi-Fi".to_string());
        }
    }

    Json(ApiResponse::ok(GetWifiRsp {
        supported,
        connected,
        ssid,
        ap_mode,
    }))
}

pub async fn connect_wifi_handler(Json(req): Json<ConnectWifiReq>) -> impl IntoResponse {
    match NetworkManager::connect_wifi(&req.ssid, &req.password).await {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => {
            error!("WiFi connect failed: {}", e);
            Json(ApiResponse::<serde_json::Value>::err(-1, &e.to_string())).into_response()
        }
    }
}

pub async fn disconnect_wifi_handler() -> impl IntoResponse {
    match NetworkManager::disconnect_wifi().await {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => {
            error!("WiFi disconnect failed: {}", e);
            Json(ApiResponse::<serde_json::Value>::err(-1, &e.to_string())).into_response()
        }
    }
}

/// Connect Wi-Fi without auth (only available while in AP mode).
pub async fn connect_wifi_no_auth_handler(Json(req): Json<ConnectWifiReq>) -> impl IntoResponse {
    let ap_mode = NetworkManager::is_wifi_ap_mode().await;
    if !ap_mode {
        return Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "wifi no-auth connect only allowed in ap mode",
        ))
        .into_response();
    }
    connect_wifi_handler(Json(req)).await.into_response()
}

// --- Ethernet Handlers ---

#[cfg(target_os = "linux")]
pub async fn get_ethernet_config_handler() -> impl IntoResponse {
    let saved = NetworkManager::read_saved_config().await;
    let current = NetworkManager::get_current_config().await;

    Json(ApiResponse::ok(network::GetEthernetConfigRsp {
        config: saved,
        current,
    }))
    .into_response()
}

#[cfg(target_os = "linux")]
pub async fn set_ethernet_config_handler(
    Json(req): Json<network::SetEthernetConfigReq>,
) -> impl IntoResponse {
    match NetworkManager::set_ethernet_config(req).await {
        Ok(_) => {
            tracing::info!("Ethernet configuration updated");
            Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
        }
        Err(e) => {
            error!("Failed to set ethernet config: {}", e);
            Json(ApiResponse::<serde_json::Value>::err(-1, &e.to_string())).into_response()
        }
    }
}
