use axum::http::StatusCode;
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
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            error!("WoL failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
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
            Json(GetMacRsp { macs }).into_response()
        }
        Err(e) => {
            error!("Failed to get WoL MACs: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

pub async fn set_wol_name_handler(Json(req): Json<SetMacNameReq>) -> impl IntoResponse {
    match NetworkManager::set_wol_mac_name(&req.mac, &req.name).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            error!("Failed to set WoL name: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

pub async fn delete_wol_mac_handler(Json(req): Json<DeleteMacReq>) -> impl IntoResponse {
    match NetworkManager::delete_wol_mac(&req.mac).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            error!("Failed to delete WoL MAC: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
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

    Json(GetWifiRsp {
        supported,
        connected,
        ssid,
        ap_mode,
    })
}

pub async fn connect_wifi_handler(Json(req): Json<ConnectWifiReq>) -> impl IntoResponse {
    match NetworkManager::connect_wifi(&req.ssid, &req.password).await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn disconnect_wifi_handler() -> impl IntoResponse {
    match NetworkManager::disconnect_wifi().await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
