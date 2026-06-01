#![allow(dead_code)]

use axum::{
    extract::{Json, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::net::SocketAddr;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

static X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
static X_REAL_IP: HeaderName = HeaderName::from_static("x-real-ip");
use crate::api::ApiResponse;
use crate::utils::{decrypt_password, get_secret_key};
use crate::config::Config;
use crate::AppState;
use bcrypt::{hash, verify, DEFAULT_COST};
use chrono::Utc;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tracing::{error, warn};

pub mod brute_force;

const COOKIE_NAME: &str = "nano-kvm-token";
const PWD_FILE: &str = "/etc/kvm/pwd";
const AUDIT_LOG_FILE: &str = "/var/log/nanokvm_auth.log";

fn build_session_cookie(token: &str, secure: bool) -> Cookie<'static> {
    let mut builder = Cookie::build((COOKIE_NAME, token.to_string()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax);
    if secure {
        builder = builder.secure(true);
    }
    builder.build()
}

fn attach_cookie(mut response: Response, cookie: Cookie<'static>) -> Response {
    if let Ok(value) = HeaderValue::from_str(&cookie.to_string()) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
    response
}

fn clear_session_cookie(secure: bool) -> Cookie<'static> {
    let mut builder = Cookie::build((COOKIE_NAME, ""))
        .path("/")
        .max_age(Duration::ZERO.try_into().unwrap())
        .http_only(true)
        .same_site(SameSite::Lax);
    if secure {
        builder = builder.secure(true);
    }
    builder.build()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub username: String,
    pub exp: usize,
    pub requires_password_change: bool,
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginRsp {
    pub token: String,
    pub requires_password_change: bool,
    pub password_expiry_days: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Account {
    pub username: String,
    pub password: String,
    pub last_password_change: Option<u64>,
    pub failed_attempts: u8,
    pub locked_until: Option<u64>,
    pub must_change_password: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub timestamp: String,
    pub event_type: String,
    pub username: String,
    pub ip_address: Option<String>,
    pub success: bool,
    pub details: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordReq {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct IsPasswordUpdatedRsp {
    #[serde(rename = "isUpdated")]
    pub is_updated: bool,
}

#[derive(Debug, Serialize)]
pub struct GetAccountRsp {
    pub username: String,
}

#[derive(Debug, Serialize)]
pub struct EncryptionKeyRsp {
    pub key: String,
}

pub async fn get_account() -> Account {
    if let Ok(content) = fs::read_to_string(PWD_FILE).await {
        if let Ok(account) = serde_json::from_str::<Account>(&content) {
            return account;
        }
    }

    let hashed_password = hash("admin", DEFAULT_COST).unwrap_or_else(|_| {
        "$2b$12$LQv3c1yqBWVHxkd0LHAkCOYz6TtxMQJqhN8/X4.nY.yx7xLw0t7i".to_string()
    });

    Account {
        username: "admin".to_string(),
        password: hashed_password,
        last_password_change: None,
        failed_attempts: 0,
        locked_until: None,
        must_change_password: true,
    }
}

async fn save_account(account: &Account) -> anyhow::Result<()> {
    let content = serde_json::to_string(account)?;
    fs::write(PWD_FILE, content).await?;
    Ok(())
}

/// Resolve client IP for brute-force tracking.
/// Default: socket peer only. Forwarded headers are honored only when `trustForwardedHeaders` is set in config.
pub fn client_ip(
    headers: &HeaderMap,
    peer: Option<SocketAddr>,
    trust_forwarded: bool,
) -> String {
    if trust_forwarded {
        if let Some(h) = headers.get(&X_FORWARDED_FOR).and_then(|h| h.to_str().ok()) {
            if let Some(ip) = h.split(',').next().map(str::trim).filter(|s| !s.is_empty()) {
                return ip.to_string();
            }
        }
        if let Some(h) = headers.get(&X_REAL_IP).and_then(|h| h.to_str().ok()) {
            let ip = h.trim();
            if !ip.is_empty() {
                return ip.to_string();
            }
        }
    }
    peer.map(|a| a.ip().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(unix)]
use axum::extract::ConnectInfo;

#[cfg(unix)]
pub async fn login_handler(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> axum::response::Response {
    login_handler_inner(state, headers, req, Some(peer)).await
}

/// Windows/dev host builds use a stub; production devices build for Linux.
#[cfg(not(unix))]
pub async fn login_handler(
    State(_state): State<Arc<AppState>>,
    Json(_req): Json<LoginReq>,
) -> Response {
    Json(ApiResponse::<()>::err(
        crate::api::error_codes::GENERIC,
        "login is only available on the device (linux) target",
    ))
    .into_response()
}

async fn login_handler_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    req: LoginReq,
    peer: Option<SocketAddr>,
) -> axum::response::Response {
    let ip_address = client_ip(
        &headers,
        peer,
        state.config.security.trust_forwarded_headers,
    );

    if state.config.authentication == "disable" {
        return Json(ApiResponse::ok(LoginRsp {
            token: "disabled".to_string(),
            requires_password_change: false,
            password_expiry_days: None,
        }))
        .into_response();
    }

    // === Brute force check (P0 security parity, now via AppState) ===
    // Note: ip_address comes from potentially untrusted headers (see get_session_ip).
    if let Some((code, msg)) = state.brute_force.check(&ip_address) {
        // Match Go behavior: sleep on locked account
        tokio::time::sleep(Duration::from_secs(3)).await;
        log_audit_event(
            "login",
            &req.username,
            false,
            &msg,
            Some(ip_address.clone()),
        )
        .await;
        return Json(ApiResponse::<()>::err(code, &msg)).into_response();
    }

    let account = get_account().await;

    let plain_password = decrypt_password(&req.password).unwrap_or(req.password.clone());

    let password_valid = verify(&plain_password, &account.password).unwrap_or(false);
    let username_matches: bool = req
        .username
        .as_bytes()
        .ct_eq(account.username.as_bytes())
        .into();

    if !password_valid || !username_matches {
        // Record failure + possible lockout (now via AppState)
        let lockout_err = state.brute_force.record_failure(&ip_address).await;

        // Match Go timing attack mitigation
        tokio::time::sleep(Duration::from_secs(2)).await;

        log_audit_event(
            "login",
            &req.username,
            false,
            "Invalid username or password",
            Some(ip_address.clone()),
        )
        .await;

        if let Some((code, msg)) = lockout_err {
            return Json(ApiResponse::<()>::err(code, &msg)).into_response();
        }

        return Json(ApiResponse::<()>::err(
            crate::api::error_codes::AUTH,
            "Invalid username or password",
        ))
        .into_response();
    }

    // Success path
    state.brute_force.clear(&ip_address).await;

    let session_id = uuid::Uuid::new_v4().to_string();
    let exp =
        Utc::now() + chrono::Duration::seconds(state.config.jwt.refresh_token_duration as i64);

    let claims = Claims {
        username: req.username.clone(),
        exp: exp.timestamp() as usize,
        requires_password_change: account.must_change_password,
        session_id,
    };

    let signing_key = state.jwt_secret.read().clone();
    match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(signing_key.as_bytes()),
    ) {
        Ok(token) => {
            let secure = state.config.proto.eq_ignore_ascii_case("https");
            let cookie = build_session_cookie(&token, secure);

            log_audit_event(
                "login",
                &req.username,
                true,
                "Login successful",
                Some(ip_address),
            )
            .await;

            attach_cookie(
                Json(ApiResponse::ok(LoginRsp {
                    token,
                    requires_password_change: account.must_change_password,
                    password_expiry_days: None,
                }))
                .into_response(),
                cookie,
            )
        }
        Err(_) => Json(ApiResponse::<()>::err(
            crate::api::error_codes::GENERIC,
            "Token generation failed",
        ))
        .into_response(),
    }
}

pub async fn logout_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if state.config.jwt.revoke_tokens_on_logout {
        let new_secret = crate::config::generate_jwt_secret_key();
        *state.jwt_secret.write() = new_secret.clone();
        let mut disk_config = Config::load().await;
        disk_config.jwt.secret_key = new_secret;
        if let Err(e) = disk_config.save().await {
            warn!("logout: failed to persist rotated JWT secret: {}", e);
        }
    }

    let secure = state.config.proto.eq_ignore_ascii_case("https");
    let cookie = clear_session_cookie(secure);
    attach_cookie(
        Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        cookie,
    )
}

#[cfg(target_os = "linux")]
async fn change_root_password(password: &str) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;

    let mut child = Command::new("passwd")
        .arg("root")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn passwd: {}", e))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "passwd stdin unavailable".to_string())?;

    stdin
        .write_all(format!("{}\n", password).as_bytes())
        .await
        .map_err(|e| format!("passwd write failed: {}", e))?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    stdin
        .write_all(format!("{}\n", password).as_bytes())
        .await
        .map_err(|e| format!("passwd confirm failed: {}", e))?;
    drop(stdin);

    let status = child
        .wait()
        .await
        .map_err(|e| format!("passwd wait failed: {}", e))?;
    if status.success() {
        Ok(())
    } else {
        Err("passwd root failed".to_string())
    }
}

#[cfg(not(target_os = "linux"))]
async fn change_root_password(_password: &str) -> Result<(), String> {
    Ok(())
}

pub async fn change_password_handler(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<ChangePasswordReq>,
) -> impl IntoResponse {
    let account = get_account().await;
    if req.username.as_bytes().ct_ne(account.username.as_bytes()).into() {
        return Json(ApiResponse::<()>::err(
            crate::api::error_codes::AUTH,
            "Invalid username",
        ))
        .into_response();
    }

    let new_plain = match decrypt_password(&req.password) {
        Ok(p) if !p.is_empty() => p,
        _ => {
            return Json(ApiResponse::<()>::err(
                crate::api::error_codes::VALIDATION,
                "invalid password",
            ))
            .into_response();
        }
    };

    let hashed = match hash(new_plain.as_str(), DEFAULT_COST) {
        Ok(h) => h,
        Err(_) => {
            return Json(ApiResponse::<()>::err(
                crate::api::error_codes::GENERIC,
                "failed to hash password",
            ))
            .into_response();
        }
    };

    let mut updated = account;
    updated.username = req.username;
    updated.password = hashed;
    updated.must_change_password = false;
    updated.last_password_change = Some(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    );

    if let Err(e) = save_account(&updated).await {
        return Json(ApiResponse::<()>::err(
            crate::api::error_codes::GENERIC,
            &format!("failed to save password: {}", e),
        ))
        .into_response();
    }

    if let Err(e) = change_root_password(&new_plain).await {
        let _ = tokio::fs::remove_file(PWD_FILE).await;
        return Json(ApiResponse::<()>::err(
            crate::api::error_codes::GENERIC,
            &format!("failed to change root password: {}", e),
        ))
        .into_response();
    }

    Json(ApiResponse::ok(())).into_response()
}

pub async fn is_password_updated_handler() -> impl IntoResponse {
    if !Path::new(PWD_FILE).exists() {
        return Json(ApiResponse::ok(IsPasswordUpdatedRsp { is_updated: false })).into_response();
    }

    let account = get_account().await;
    // Match Go: updated when stored hash no longer matches default "admin" password.
    let is_updated = !verify("admin", &account.password).unwrap_or(false);
    Json(ApiResponse::ok(IsPasswordUpdatedRsp { is_updated })).into_response()
}

pub async fn get_account_handler() -> impl IntoResponse {
    let account = get_account().await;
    Json(ApiResponse::ok(GetAccountRsp {
        username: account.username,
    }))
}

/// Return the device-specific encryption key for frontend password encryption.
/// This endpoint is public (no auth required) since the key is needed before login.
pub async fn get_encryption_key_handler() -> impl IntoResponse {
    match get_secret_key() {
        Ok(key) => Json(ApiResponse::ok(EncryptionKeyRsp { key })).into_response(),
        Err(_) => Json(ApiResponse::<()>::err(
            crate::api::error_codes::GENERIC,
            "Failed to get encryption key",
        ))
        .into_response(),
    }
}

/// Routes a must-change-password session may access (everything else requires a normal login).
fn allows_during_password_change(path: &str) -> bool {
    const ALLOWED: &[&str] = &[
        "/api/auth/password",
        "/api/auth/account",
        "/api/auth/logout",
        "/api/logout",
    ];
    ALLOWED.iter().any(|allowed| path == *allowed)
}

pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if state.config.authentication == "disable" {
        return next.run(req).await;
    }

    let token = jar
        .get(COOKIE_NAME)
        .map(|c| c.value().to_string())
        .or_else(|| {
            req.headers()
                .get(header::AUTHORIZATION)
                .and_then(|h: &axum::http::HeaderValue| h.to_str().ok())
                .and_then(|s: &str| s.strip_prefix("Bearer "))
                .map(|s: &str| s.to_string())
        });

    if let Some(token) = token {
        let signing_key = state.jwt_secret.read().clone();
        let validation = Validation::new(Algorithm::HS256);
        if let Ok(data) = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(signing_key.as_bytes()),
            &validation,
        ) {
            if !data.claims.requires_password_change || allows_during_password_change(req.uri().path())
            {
                return next.run(req).await;
            }
            return (StatusCode::FORBIDDEN, "Password change required").into_response();
        }
    }

    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}

pub async fn log_audit_event(
    event_type: &str,
    username: &str,
    success: bool,
    details: &str,
    ip_address: Option<String>,
) {
    let timestamp = Utc::now().to_rfc3339();
    let entry = AuditLogEntry {
        timestamp,
        event_type: event_type.to_string(),
        username: username.to_string(),
        ip_address,
        success,
        details: details.to_string(),
    };

    if let Ok(json) = serde_json::to_string(&entry) {
        let log_line = format!("{}\n", json);

        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(AUDIT_LOG_FILE)
            .await
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(log_line.as_bytes()).await {
                    error!("Failed to write audit log: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to open audit log file: {}", e);
            }
        }
    }
}

pub async fn generate_token(
    state: &Arc<AppState>,
    username: &str,
    requires_password_change: bool,
) -> Result<String, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let exp =
        Utc::now() + chrono::Duration::seconds(state.config.jwt.refresh_token_duration as i64);

    let claims = Claims {
        username: username.to_string(),
        exp: exp.timestamp() as usize,
        requires_password_change,
        session_id,
    };

    let signing_key = state.jwt_secret.read().clone();
    match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(signing_key.as_bytes()),
    ) {
        Ok(token) => Ok(token),
        Err(_) => Err("Token generation failed".to_string()),
    }
}

// Brute force protection is now properly implemented in auth/brute_force.rs
// and wired through AppState.brute_force (see main.rs).
