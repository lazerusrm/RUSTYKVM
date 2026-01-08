use axum::{
    extract::{Json, Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::process::Command;
use tracing::{error, info, warn};

use super::cbor::{read_bytes, read_int};
use crate::auth::{get_account, log_audit_event};
use crate::passkey::{
    crypto::AuthenticatorData,
    models::{
        CoseKey, LoginChallengeResponse, RecoverResponse, RecoveryCodesResponse, SetupResponse,
        VerifyResponse, PASSKEYS_FILE, RECOVERY_CODES_FILE,
    },
    qr::generate_qr_code_simple,
    recovery::{
        format_recovery_codes_for_display, generate_recovery_codes, save_recovery_codes,
        validate_and_consume_code,
    },
    PasskeyState,
};
use crate::system::capabilities::Capabilities;
use crate::AppState;

fn extract_public_key_from_attestation(attestation_cbor: &[u8]) -> Option<CoseKey> {
    if attestation_cbor.len() < 37 || attestation_cbor[0] != 0xa3 {
        return None;
    }

    let mut offset = 1;
    let mut auth_data_offset = None;

    while offset < attestation_cbor.len() {
        let key = read_int(attestation_cbor, &mut offset)?;

        match key {
            2 => {
                if let Some(value) = read_bytes(attestation_cbor, &mut offset) {
                    auth_data_offset = Some(offset - value.len() - 1);
                }
            }
            _ => {
                let _ = read_bytes(attestation_cbor, &mut offset);
            }
        }
    }

    let auth_data = &attestation_cbor[auth_data_offset?..];
    if auth_data.len() < 37 {
        return None;
    }

    let flags = auth_data[32];
    let has_attested_credential = (flags & 0x40) != 0;

    let mut pk_offset = 37;
    if has_attested_credential {
        if auth_data.len() <= pk_offset + 16 {
            return None;
        }
        pk_offset += 16;
    }

    if pk_offset >= auth_data.len() {
        return None;
    }

    CoseKey::from_cbor(&auth_data[pk_offset..])
}

#[derive(Serialize, Deserialize)]
pub struct PasskeySetupRequest {
    pub device_name: Option<String>,
}

async fn check_passkey_exists() -> bool {
    tokio::fs::metadata(PASSKEYS_FILE).await.is_ok()
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

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }
    None
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

pub async fn passkey_setup_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasskeySetupRequest>,
) -> impl IntoResponse {
    info!("Passkey setup requested, device: {:?}", req.device_name);

    let caps = detect_capabilities().await;

    if !caps.tailscale_installed {
        warn!("Tailscale not installed");
        return Json(SetupResponse {
            success: false,
            funnel_url: String::new(),
            enrollment_url: String::new(),
            qr_code: String::new(),
            expires_at: String::new(),
        });
    }

    if !caps.tailscale_connected {
        warn!("Tailscale not connected");
        return Json(SetupResponse {
            success: false,
            funnel_url: String::new(),
            enrollment_url: String::new(),
            qr_code: String::new(),
            expires_at: String::new(),
        });
    }

    let rp_id = if caps.tailscale_funnel_active {
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

        match wait_for_funnel_ready(30).await {
            Some(url) => url,
            None => {
                error!("Failed to get funnel URL after enabling");
                return Json(SetupResponse {
                    success: false,
                    funnel_url: String::new(),
                    enrollment_url: String::new(),
                    qr_code: String::new(),
                    expires_at: String::new(),
                });
            }
        }
    };

    let challenge_id = PasskeyState::generate_challenge_id();
    let challenge = generate_random_challenge();
    let user_id = generate_user_id();

    {
        let mut pending = state.passkey_state.pending_challenge.lock().await;
        *pending = Some(state.passkey_state.new_enrollment_challenge(
            challenge_id.clone(),
            challenge.clone(),
            user_id.clone(),
            rp_id.clone(),
            None,
        ));
    }

    let enrollment_url = format!("{}/passkey/enroll/{}", rp_id, challenge_id);
    let qr_code = match generate_qr_code_simple(&enrollment_url) {
        Ok(qr) => qr,
        Err(e) => {
            error!("Failed to generate QR code: {}", e);
            return Json(SetupResponse {
                success: false,
                funnel_url: String::new(),
                enrollment_url: String::new(),
                qr_code: String::new(),
                expires_at: String::new(),
            });
        }
    };

    let expires_at = chrono::Utc::now() + chrono::Duration::minutes(5);

    info!("Setup initiated: funnel={}, enrollment_url={}", rp_id, enrollment_url);

    Json(SetupResponse {
        success: true,
        funnel_url: rp_id,
        enrollment_url,
        qr_code,
        expires_at: expires_at.to_rfc3339(),
    })
}

fn generate_random_challenge() -> String {
    let mut challenge = [0u8; 32];
    rand::thread_rng().fill(&mut challenge);
    STANDARD.encode(&challenge)
}

fn generate_user_id() -> Vec<u8> {
    b"admin".to_vec()
}

pub async fn enroll_complete_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
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

    let credential_id = req.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    if credential_id.is_empty() {
        warn!("Missing credential ID in enrollment");
        return Json(RecoveryCodesResponse {
            success: false,
            codes: Vec::new(),
        });
    }

    let client_data_json = req.get("response")
        .and_then(|r| r.get("clientDataJSON"))
        .and_then(|v| v.as_str())
        .map(|s| String::from_utf8(STANDARD.decode(s).unwrap_or_default()).unwrap_or_default())
        .unwrap_or_default();

    let attestation_object = req.get("response")
        .and_then(|r| r.get("attestationObject"))
        .and_then(|v| v.as_str())
        .map(|s| STANDARD.decode(s).unwrap_or_default())
        .unwrap_or_default();

    if attestation_object.is_empty() {
        warn!("Missing attestation object in enrollment");
        return Json(RecoveryCodesResponse {
            success: false,
            codes: Vec::new(),
        });
    }

    let cose_key = extract_public_key_from_attestation(&attestation_object);
    if cose_key.is_none() {
        warn!("Failed to parse COSE key from attestation");
        return Json(RecoveryCodesResponse {
            success: false,
            codes: Vec::new(),
        });
    }
    let cose_key = cose_key.unwrap();

    let mut storage = load_passkeys().await.unwrap_or_default();

    let device_name = req.get("deviceName")
        .and_then(|v| v.as_str().map(|s| s.to_string()));

    let credential = crate::passkey::models::PasskeyCredential {
        id: credential_id.clone(),
        public_key: cose_key,
        counter: 0,
        transports: vec!["hybrid".to_string()],
        created: chrono::Utc::now(),
        device_name,
        rp_id: challenge.rp_id,
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

async fn load_passkeys() -> std::io::Result<crate::passkey::models::PasskeyStorage> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(metadata) = tokio::fs::metadata(PASSKEYS_FILE).await {
            if let Ok(perms) = metadata.permissions().mode() {
                if perms & 0o777 != 0o600 {
                    warn!("Passkeys file has incorrect permissions: {:o}", perms);
                }
            }
        }
    }

    match tokio::fs::read_to_string(PASSKEYS_FILE).await {
        Ok(content) => {
            serde_json::from_str(&content).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        }
        Err(e) => Err(e),
    }
}

async fn save_passkeys(storage: &crate::passkey::models::PasskeyStorage) -> std::io::Result<()> {
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

#[derive(Deserialize)]
pub struct LoginChallengeRequest {
    pub credential_id: Option<String>,
}

pub async fn login_challenge_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginChallengeRequest>,
) -> impl IntoResponse {
    let rp_id = get_funnel_url().await
        .unwrap_or_else(|| "nanokvm".to_string());

    let challenge_id = PasskeyState::generate_challenge_id();
    let challenge = generate_random_challenge();

    let mut pending = state.passkey_state.pending_challenge.lock().await;
    *pending = Some(state.passkey_state.new_login_challenge(
        challenge_id.clone(),
        challenge.clone(),
        rp_id.clone(),
        req.credential_id,
    ));

    info!("Login challenge generated: id={}, rp_id={}", challenge_id, rp_id);

    Json(LoginChallengeResponse {
        challenge,
        challenge_id,
        rp_id,
        timeout: 300000,
    })
}

pub async fn login_verify_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    info!("Passkey login verification requested");

    let pending_challenge = {
        let pending = state.passkey_state.pending_challenge.lock().await;
        match &*pending {
            Some(c) if !c.is_expired() && !c.is_enrollment => Some(c.clone()),
            _ => None,
        }
    };

    let challenge = match pending_challenge {
        Some(c) => c,
        None => {
            warn!("No valid login challenge found");
            return Json(VerifyResponse {
                success: false,
                token: None,
                requires_password_change: None,
                error: Some("Invalid or expired challenge".to_string()),
            });
        }
    };

    const MAX_CREDENTIAL_ID_LENGTH: usize = 256;

    let credential_id = req.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    if credential_id.is_empty() || credential_id.len() > MAX_CREDENTIAL_ID_LENGTH {
        warn!("Invalid credential ID length: {}", credential_id.len());
        return Json(VerifyResponse {
            success: false,
            token: None,
            requires_password_change: None,
            error: Some("Invalid credential".to_string()),
        });
    }

    let client_data_json = req.get("response")
        .and_then(|r| r.get("clientDataJSON"))
        .and_then(|v| v.as_str())
        .map(|s| String::from_utf8(STANDARD.decode(s).unwrap_or_default()).unwrap_or_default())
        .unwrap_or_default();

    let authenticator_data_str = req.get("response")
        .and_then(|r| r.get("authenticatorData"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    let authenticator_data = STANDARD.decode(&authenticator_data_str).unwrap_or_default();

    let signature_str = req.get("response")
        .and_then(|r| r.get("signature"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    let signature = STANDARD.decode(&signature_str).unwrap_or_default();

    if authenticator_data.is_empty() || signature.is_empty() {
        warn!("Missing authenticator data or signature");
        return Json(VerifyResponse {
            success: false,
            token: None,
            requires_password_change: None,
            error: Some("Invalid assertion data".to_string()),
        });
    }

    let storage = load_passkeys().await;
    let credential = match storage {
        Ok(s) => s.credentials.iter().find(|c| c.id == credential_id),
        Err(e) => {
            error!("Failed to load passkeys: {}", e);
            None
        }
    };

    if let Some(ref expected_cred_id) = challenge.credential_id {
        if &credential_id != expected_cred_id {
            warn!("Credential ID mismatch: expected {}, got {}", expected_cred_id, credential_id);
            return Json(VerifyResponse {
                success: false,
                token: None,
                requires_password_change: None,
                error: Some("Invalid credential for this challenge".to_string()),
            });
        }
    }

    let (auth_data, counter) = match credential {
        Some(c) => {
            let auth_data = AuthenticatorData::parse(&authenticator_data);
            match auth_data {
                Some(ad) => {
                    if ad.counter <= c.counter {
                        warn!("Credential counter regression: {} <= {}", ad.counter, c.counter);
                        return Json(VerifyResponse {
                            success: false,
                            token: None,
                            requires_password_change: None,
                            error: Some("Credential may have been cloned".to_string()),
                        });
                    }
                    (ad, ad.counter)
                }
                None => {
                    warn!("Failed to parse authenticator data");
                    return Json(VerifyResponse {
                        success: false,
                        token: None,
                        requires_password_change: None,
                        error: Some("Invalid authenticator data".to_string()),
                    });
                }
            }
        }
        None => {
            warn!("Unknown passkey credential: {}", credential_id);
            return Json(VerifyResponse {
                success: false,
                token: None,
                requires_password_change: None,
                error: Some("Unknown credential".to_string()),
            });
        }
    };

    if (auth_data.flags & 0x01) == 0 {
        warn!("User verification not performed");
        return Json(VerifyResponse {
            success: false,
            token: None,
            requires_password_change: None,
            error: Some("User verification required".to_string()),
        });
    }

    let client_data_hash = {
        let parsed: Result<serde_json::Map<String, serde_json::Value>, _> = serde_json::from_str(&client_data_json);
        match parsed {
            Ok(map) => {
                let challenge_b64 = map.get("challenge").and_then(|v| v.as_str()).unwrap_or("");
                let origin = map.get("origin").and_then(|v| v.as_str()).unwrap_or("");
                let type_ = map.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let cross_origin = map.get("crossOrigin").and_then(|v| v.as_bool()).unwrap_or(false);

                if challenge_b64 != challenge.challenge {
                    warn!("Challenge mismatch: expected {}, got {}", challenge.challenge, challenge_b64);
                    return Json(VerifyResponse {
                        success: false,
                        token: None,
                        requires_password_change: None,
                        error: Some("Invalid challenge".to_string()),
                    });
                }

                let expected_origin = format!("https://{}", challenge.rp_id);
                if origin != expected_origin {
                    warn!("Origin mismatch: expected {}, got {}", expected_origin, origin);
                    return Json(VerifyResponse {
                        success: false,
                        token: None,
                        requires_password_change: None,
                        error: Some("Invalid origin".to_string()),
                    });
                }

                let json = format!(r#"{{"type":"{}","challenge":"{}","origin":"{}","crossOrigin":{}}}"#,
                    type_, challenge_b64, origin, cross_origin);
                sha2::Sha256::digest(json.as_bytes()).to_vec()
            }
            Err(_) => {
                warn!("Failed to parse client data");
                return Json(VerifyResponse {
                    success: false,
                    token: None,
                    requires_password_change: None,
                    error: Some("Invalid client data".to_string()),
                });
            }
        }
    };

    let mut signed_data = authenticator_data;
    signed_data.extend(client_data_hash);

    match credential {
        Some(c) => {
            if !c.public_key.verify_signature(&signed_data, &signature) {
                warn!("Signature verification failed");
                return Json(VerifyResponse {
                    success: false,
                    token: None,
                    requires_password_change: None,
                    error: Some("Signature verification failed".to_string()),
                });
            }

            let mut storage = match load_passkeys().await {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to reload storage: {}", e);
                    return Json(VerifyResponse {
                        success: false,
                        token: None,
                        requires_password_change: None,
                        error: Some("Internal error".to_string()),
                    });
                }
            };

            if let Some(cred) = storage.credentials.iter_mut().find(|c| c.id == credential_id) {
                cred.counter = counter;
            }

            if let Err(e) = save_passkeys(&storage).await {
                error!("Failed to update counter: {}", e);
            }
        }
        None => {
            return Json(VerifyResponse {
                success: false,
                token: None,
                requires_password_change: None,
                error: Some("Credential not found".to_string()),
            });
        }
    }

    let mut pending = state.passkey_state.pending_challenge.lock().await;
    *pending = None;
    drop(pending);

    info!("Passkey verified successfully: id={}, counter={}", credential_id, counter);

    let account = get_account().await;
    match crate::auth::generate_token(&state, &account.username, account.must_change_password).await {
        Ok(token) => {
            log_audit_event("PASSKEY_LOGIN_SUCCESS", &account.username, true, "Passkey login successful", None).await;
            Json(VerifyResponse {
                success: true,
                token: Some(token),
                requires_password_change: Some(account.must_change_password),
                error: None,
            })
        }
        Err(e) => {
            error!("Failed to generate JWT token: {}", e);
            Json(VerifyResponse {
                success: false,
                token: None,
                requires_password_change: None,
                error: Some("Token generation failed".to_string()),
            })
        }
    }
}

pub async fn recover_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
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
            let account = get_account().await;
            match crate::auth::generate_token(&state, &account.username, account.must_change_password).await {
                Ok(token) => {
                    log_audit_event("RECOVERY_SUCCESS", &account.username, true, "Recovery code login successful", None).await;
                    Json(RecoverResponse {
                        success: true,
                        token: Some(token),
                        remaining_codes: Some(remaining),
                        error: None,
                    })
                }
                Err(e) => {
                    error!("Failed to generate JWT token: {}", e);
                    Json(RecoverResponse {
                        success: false,
                        token: None,
                        remaining_codes: None,
                        error: Some("Token generation failed".to_string()),
                    })
                }
            }
        }
        Ok((false, _)) => {
            warn!("Invalid recovery code: {}", code);
            log_audit_event("RECOVERY_FAILED", "unknown", false, "Invalid recovery code attempted", None).await;
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
            let parse_result: Result<serde_json::Value, _> = serde_json::from_str(&content);
            match parse_result {
                Ok(codes) => {
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
                Err(e) => {
                    error!("Failed to parse recovery codes: {}", e);
                    "Failed to load recovery codes".to_string()
                }
            }
        }
        Err(_) => "No recovery codes found".to_string(),
    }
}

#[derive(Deserialize)]
pub struct QrQuery {
    pub text: String,
}

pub async fn qr_code_handler(Query(query): Query<QrQuery>) -> impl IntoResponse {
    if query.text.is_empty() {
        return (StatusCode::BAD_REQUEST, "Missing text parameter").into_response();
    }

    const MAX_QR_LENGTH: usize = 2048;
    if query.text.len() > MAX_QR_LENGTH {
        return (StatusCode::BAD_REQUEST, "Text too long for QR code").into_response();
    }

    if !query.text.starts_with("https://") {
        return (StatusCode::BAD_REQUEST, "Invalid URL format").into_response();
    }

    match generate_qr_code_simple(&query.text) {
        Ok(qr_data) => {
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
