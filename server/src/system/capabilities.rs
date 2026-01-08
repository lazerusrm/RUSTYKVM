use axum::{
    extract::State,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;
use tokio::process::Command;
use serde::{Serialize, Deserialize};

use crate::AppState;

pub const PASSKEYS_FILE: &str = "/etc/kvm/passkeys.json";

#[derive(Debug, Serialize)]
pub struct Capabilities {
    pub tailscale_installed: bool,
    pub tailscale_connected: bool,
    pub tailscale_funnel_active: bool,
    pub funnel_url: Option<String>,
    pub passkey_configured: bool,
    pub passkey_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CapabilitiesResponse {
    pub tailscale_installed: bool,
    pub tailscale_connected: bool,
    pub tailscale_funnel_active: bool,
    pub funnel_url: Option<String>,
    pub passkey_configured: bool,
    pub passkey_reason: Option<String>,
}

pub async fn get_capabilities_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let caps = detect_capabilities().await;
    Json(caps)
}

async fn detect_capabilities() -> Capabilities {
    let tailscale_installed = check_tailscale_installed().await;
    let tailscale_connected = check_tailscale_connected().await;
    let funnel_active = check_funnel_active().await;
    let passkey_exists = check_passkey_exists().await;
    
    let funnel_url = if funnel_active {
        get_funnel_url().await
    } else {
        None
    };
    
    let (passkey_configured, reason) = if !funnel_active {
        (false, Some("funnel_not_active".to_string()))
    } else if !passkey_exists {
        (false, Some("no_passkey_configured".to_string()))
    } else {
        (true, None)
    };
    
    Capabilities {
        tailscale_installed,
        tailscale_connected,
        tailscale_funnel_active: funnel_active,
        funnel_url,
        passkey_configured,
        passkey_reason: reason,
    }
}

async fn check_tailscale_installed() -> bool {
    Command::new("which")
        .arg("tailscale")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn check_tailscale_connected() -> bool {
    let output = Command::new("tailscale")
        .args(&["status", "--json"])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());
    
    if let Some(json) = output {
        json.contains("\"Online\": true") || (json.contains("\"Self\":{") && json.contains("\"Online\":true"))
    } else {
        false
    }
}

async fn check_funnel_active() -> bool {
    let output = Command::new("tailscale")
        .args(&["serve", "status"])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());
    
    if let Some(status) = output {
        status.contains("https://") || status.contains("forwarding")
    } else {
        false
    }
}

async fn get_funnel_url() -> Option<String> {
    let output = Command::new("tailscale")
        .args(&["serve", "status"])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());
    
    if let Some(status) = output {
        if let Some(start) = status.find("https://") {
            let url_part = &status[start..];
            let end = url_part.find(|c| c == '\n' || c == ' ').unwrap_or(url_part.len());
            Some(url_part[..end].to_string())
        } else {
            None
        }
    } else {
        None
    }
}

async fn check_passkey_exists() -> bool {
    tokio::fs::metadata(PASSKEYS_FILE).await.is_ok()
}
