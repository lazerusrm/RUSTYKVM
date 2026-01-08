use axum::{
    extract::{State, Json, Extension, Query},
    response::{IntoResponse, Response},
    http::{StatusCode, header},
};
use std::sync::Arc;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use base64::{Engine as _, engine::general_purpose::STANDARD, URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error, debug};
use uuid::Uuid;

use crate::AppState;
use crate::passkey::{
    models::{
        PasskeyStorage, PasskeyCredential, PasskeyResponse, SetupResponse, VerifyResponse, RecoverResponse,
        RecoveryCodesResponse, LoginChallengeResponse, EnrollmentStartResponse, PASSKEYS_FILE, RECOVERY_CODES_FILE,
    },
    recovery::{generate_recovery_codes, save_recovery_codes, validate_and_consume_code, format_recovery_codes_for_display},
    qr::generate_qr_code_simple,
    PasskeyState, PASSKEYS_FILE as _,
};

#[derive(Serialize, Deserialize)]
pub struct PasskeySetupRequest {
    pub device_name: Option<String>,
}

pub async fn get_capabilities_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let caps = detect_capabilities().await;
    Json(caps)
}

#[derive(Debug, Serialize)]
pub struct Capabilities {
    pub tailscale_installed: bool,
    pub tailscale_connected: bool,
    pub tailscale_funnel_active: bool,
    pub funnel_url: Option<String>,
    pub passkey_configured: bool,
    pub passkey_reason: Option<String>,
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
        // Parse JSON to check if Self.Online is true
        // Simple check: look for "Online": true in the output
        json.contains("\"Online\": true") || json.contains("\"Self\":{") && json.contains("\"Online\":true")
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
        // Parse URL from status output
        // Format: https://<device>.<tailnet>.ts.net
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

pub async fn passkey_setup_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasskeySetupRequest>,
) -> impl IntoResponse {
    info!("Passkey setup requested");
    
    let caps = detect_capabilities().await;
    
    if !caps.tailscale_installed {
        return Json(SetupResponse {
            success: false,
            funnel_url: String::new(),
            enrollment_url: String::new(),
            qr_code: String::new(),
            expires_at: String::new(),
        });
    }
    
    if !caps.tailscale_connected {
        error!("Tailscale is installed but not connected");
        return Json(SetupResponse {
            success: false,
            funnel_url: String::new(),
            enrollment_url: String::new(),
            qr_code: String::new(),
            expires_at: String::new(),
        });
    }
    
    let funnel_url = if caps.tailscale_funnel_active {
        caps.funnel_url.clone()
    } else {
        info!("Enabling Tailscale funnel...");
        let enable_result = enable_tailscale_funnel().await;
        if !enable_result {
            error!("Failed to enable Tailscale funnel");
            return Json(SetupResponse {
                success: false,
                funnel_url: String::new(),
                enrollment_url: String::new(),
                qr_code: String::new(),
                expires_at: String::new(),
            });
        }
        
        wait_for_funnel_ready(30).await
    };
    
    let funnel_url = match funnel_url {
        Some(url) => url,
        None => {
            error!("Failed to get funnel URL");
            return Json(SetupResponse {
                success: false,
                funnel_url: String::new(),
                enrollment_url: String::new(),
                qr_code: String::new(),
                expires_at: String::new(),
            });
        }
    };
    
    let challenge_id = PasskeyState::generate_challenge_id();
    let challenge = generate_random_challenge();
    let user_id = generate_user_id();
    
    let pending = state.passkey_state.pending_challenge.lock().await;
    *pending = Some(state.passkey_state.new_enrollment_challenge(
        challenge_id.clone(),
        challenge.clone(),
        user_id.clone(),
    ));
    drop(pending);
    
    let enrollment_url = format!("{}/passkey/enroll/{}", funnel_url, challenge_id);
    let qr_code = match generate_qr_code_simple(&enrollment_url) {
        Ok(qr) => qr,
        Err(e) => {
            error!("Failed to generate QR code: {}", e);
            String::new()
        }
    };
    
    let expires_at = chrono::Utc::now() + chrono::Duration::minutes(5);
    
    info!("Setup initiated: funnel={}, enrollment_url={}", funnel_url, enrollment_url);
    
    Json(SetupResponse {
        success: true,
        funnel_url,
        enrollment_url,
        qr_code,
        expires_at: expires_at.to_rfc3339(),
    })
}

async fn enable_tailscale_funnel() -> bool {
    let output = Command::new("tailscale")
        .args(&["serve", "https", "localhost:8443"])
        .output()
        .await;
    
    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

async fn wait_for_funnel_ready(max_seconds: u64) -> Option<String> {
    for _ in 0..max_seconds {
        let status = Command::new("tailscale")
            .args(&["serve", "status"])
            .output()
            .await
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok());
        
        if let Some(s) = status {
            if let Some(start) = s.find("https://") {
                let end = s[start..].find(|c| c == '\n' || c == ' ').unwrap_or(s[start..].len());
                let url = &s[start..start + end];
                if url.contains(".ts.net") {
                    return Some(url.to_string());
                }
            }
        }
        
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    None
}

fn generate_random_challenge() -> Vec<u8> {
    let mut challenge = [0u8; 32];
    rand::thread_rng().fill(&mut challenge);
    challenge.to_vec()
}

fn generate_user_id() -> Vec<u8> {
    // Fixed user ID for admin user
    b"admin".to_vec()
}

pub async fn enroll_complete_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasskeyResponse>,
) -> impl IntoResponse {
    info!("Passkey enrollment completion requested");
    
    let pending = state.passkey_state.pending_challenge.lock().await;
    let challenge = match &*pending {
        Some(c) if !c.is_expired() && c.is_enrollment => c.clone(),
        _ => {
            warn!("No valid enrollment challenge found");
            return Json(RecoveryCodesResponse {
                success: false,
                codes: Vec::new(),
            });
        }
    };
    drop(pending);
    
    let credential_id = STANDARD.encode(&req.id.as_bytes());
    
    let mut storage = load_passkeys().await.unwrap_or_default();
    
    let credential = PasskeyCredential {
        id: credential_id.clone(),
        public_key: req.response.attestationObject
            .map(|a| STANDARD.decode(&a).unwrap_or_default())
            .unwrap_or_default(),
        counter: 0,
        transports: vec!["hybrid".to_string()],
        created: chrono::Utc::now(),
        device_name: None,
    };
    
    storage.credentials.push(credential);
    storage.updated_at = chrono::Utc::now();
    
    if let Err(e) = save_passkeys(&storage).await {
        error!("Failed to save passkey: {}", e);
        return Json(RecoveryCodesResponse {
            success: false,
            codes: Vec::new(),
        });
    }
    
    let recovery_storage = generate_recovery_codes(4);
    let recovery_codes = format_recovery_codes_for_display(&recovery_storage);
    
    if let Err(e) = save_recovery_codes(&recovery_storage).await {
        error!("Failed to save recovery codes: {}", e);
    }
    
    let mut pending = state.passkey_state.pending_challenge.lock().await;
    *pending = None;
    drop(pending);
    
    info!("Passkey enrolled successfully: id={}", credential_id);
    
    Json(RecoveryCodesResponse {
        success: true,
        codes: recovery_codes,
    })
}

async fn load_passkeys() -> std::io::Result<PasskeyStorage> {
    match tokio::fs::read_to_string(PASSKEYS_FILE).await {
        Ok(content) => {
            serde_json::from_str(&content).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        }
        Err(e) => Err(e),
    }
}

async fn save_passkeys(storage: &PasskeyStorage) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(PASSKEYS_FILE).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let json = serde_json::to_string_pretty(storage)?;
    tokio::fs::write(PASSKEYS_FILE, json).await?;
    
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(PASSKEYS_FILE, tokio::fs::Permissions::from_mode(0o600)).await.ok();
    }
    
    Ok(())
}

pub async fn login_challenge_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let challenge_id = PasskeyState::generate_challenge_id();
    let challenge = generate_random_challenge();
    
    let rp_id = get_funnel_url().await
        .as_ref()
        .map(|url| {
            url.replace("https://", "")
                .split('.')
                .skip(1)
                .collect::<Vec<_>>()
                .join(".")
        })
        .unwrap_or_else(|| "nanokvm".to_string());
    
    let mut pending = state.passkey_state.pending_challenge.lock().await;
    *pending = Some(state.passkey_state.new_login_challenge(
        challenge_id.clone(),
        challenge.clone(),
    ));
    drop(pending);
    
    let challenge_b64 = STANDARD.encode(&challenge);
    
    info!("Login challenge generated: id={}, rp_id={}", challenge_id, rp_id);
    
    Json(LoginChallengeResponse {
        challenge: challenge_b64,
        challenge_id,
        rp_id,
        timeout: 300000,
    })
}

pub async fn login_verify_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasskeyResponse>,
) -> impl IntoResponse {
    info!("Passkey login verification requested");
    
    let pending = state.passkey_state.pending_challenge.lock().await;
    let challenge = match &*pending {
        Some(c) if !c.is_expired() && !c.is_enrollment => c.clone(),
        _ => {
            warn!("No valid login challenge found");
            return Json(VerifyResponse {
                success: false,
                token: None,
                requires_password_change: None,
                error: Some("Invalid or expired challenge".to_string()),
            });
        }
    };
    drop(pending);
    
    let credential_id = STANDARD.encode(&req.id.as_bytes());
    
    let storage = load_passkeys().await;
    let credential = match storage {
        Ok(s) => s.credentials.iter().find(|c| c.id == credential_id),
        Err(_) => None,
    };
    
    match credential {
        Some(c) => {
            info!("Passkey verified successfully: id={}", credential_id);
            
            let mut pending = state.passkey_state.pending_challenge.lock().await;
            *pending = None;
            drop(pending);
            
            Json(VerifyResponse {
                success: true,
                token: Some("verified".to_string()),
                requires_password_change: Some(false),
                error: None,
            })
        }
        None => {
            warn!("Unknown passkey credential: {}", credential_id);
            Json(VerifyResponse {
                success: false,
                token: None,
                requires_password_change: None,
                error: Some("Unknown credential".to_string()),
            })
        }
    }
}

pub async fn recover_handler(Json(req): Json<serde_json::Value>) -> impl IntoResponse {
    let code = req.get("recovery_code")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    
    if code.is_empty() {
        return Json(RecoverResponse {
            success: false,
            token: None,
            remaining_codes: None,
            error: Some("Recovery code required".to_string()),
        });
    }
    
    match validate_and_consume_code(code).await {
        Ok((true, remaining)) => {
            info!("Recovery code validated, remaining: {}", remaining);
            Json(RecoverResponse {
                success: true,
                token: Some("recovered".to_string()),
                remaining_codes: Some(remaining),
                error: None,
            })
        }
        Ok((false, _)) => {
            warn!("Invalid recovery code: {}", code);
            Json(RecoverResponse {
                success: false,
                token: None,
                remaining_codes: None,
                error: Some("Invalid recovery code".to_string()),
            })
        }
        Err(e) => {
            error!("Recovery error: {}", e);
            Json(RecoverResponse {
                success: false,
                token: None,
                remaining_codes: None,
                error: Some("Recovery failed".to_string()),
            })
        }
    }
}

pub async fn recovery_download_handler() -> impl IntoResponse {
    match tokio::fs::read_to_string(RECOVERY_CODES_FILE).await {
        Ok(content) => {
            let codes: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
            let codes_str = codes.get("codes")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|c| c.get("code").and_then(|c| c.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_else(|| "No recovery codes found".to_string());
            
            format!("Recovery Codes\n\n{}\n\nSave these codes in a safe place.\nOne-time use only.", codes_str)
        }
        Err(_) => "No recovery codes found".to_string(),
    }
}

use axum::extract::Query;

#[derive(Deserialize)]
pub struct QrQuery {
    text: String,
}

pub async fn qr_code_handler(Query(query): Query<QrQuery>) -> impl IntoResponse {
    if query.text.is_empty() {
        return (StatusCode::BAD_REQUEST, "Missing text parameter").into_response();
    }

    match generate_qr_code_simple(&query.text) {
        Ok(qr_data) => {
            // Extract base64 data from data URL
            if let Some(base64_data) = qr_data.strip_prefix("data:image/png;base64,") {
                let image_data = match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, base64_data) {
                    Ok(data) => data,
                    Err(_) => {
                        return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to decode QR").into_response();
                    }
                };

                ([(header::CONTENT_TYPE, "image/png")], image_data).into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, "Invalid QR format").into_response()
            }
        }
        Err(e) => {
            error!("QR generation failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate QR").into_response()
        }
    }
}
