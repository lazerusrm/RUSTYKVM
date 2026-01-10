#![allow(dead_code)]

use axum::{
    extract::{Json, State},
    http::{header, HeaderMap, HeaderName, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

static X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
static X_REAL_IP: HeaderName = HeaderName::from_static("x-real-ip");
use crate::utils::{decrypt_password, get_secret_key};
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
use tracing::error;

const COOKIE_NAME: &str = "nano-kvm-token";
const PWD_FILE: &str = "/etc/kvm/pwd";
const AUDIT_LOG_FILE: &str = "/var/log/nanokvm_auth.log";

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

/// API response wrapper matching Go server format
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub msg: String,
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: 0,
            msg: "success".to_string(),
            data: Some(data),
        }
    }

    pub fn err(code: i32, msg: &str) -> ApiResponse<()> {
        ApiResponse {
            code,
            msg: msg.to_string(),
            data: None,
        }
    }
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

fn get_session_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get(&X_FORWARDED_FOR)
        .or(headers.get(&X_REAL_IP))
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

pub async fn login_handler(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    headers: HeaderMap,
    Json(req): Json<LoginReq>,
) -> impl IntoResponse {
    let _ip_address = get_session_ip(&headers);

    if state.config.authentication == "disable" {
        return (
            jar,
            Json(ApiResponse::ok(LoginRsp {
                token: "disabled".to_string(),
                requires_password_change: false,
                password_expiry_days: None,
            })),
        )
            .into_response();
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
        return Json(ApiResponse::<()>::err(-2, "Invalid username or password")).into_response();
    }

    let session_id = uuid::Uuid::new_v4().to_string();
    let exp =
        Utc::now() + chrono::Duration::seconds(state.config.jwt.refresh_token_duration as i64);

    let claims = Claims {
        username: req.username,
        exp: exp.timestamp() as usize,
        requires_password_change: account.must_change_password,
        session_id,
    };

    let secret = state.config.jwt.secret_key.as_bytes();
    match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    ) {
        Ok(token) => {
            let cookie = Cookie::build((COOKIE_NAME, token.clone()))
                .path("/")
                .http_only(false)  // Must be false so frontend JS can read it for route protection
                .same_site(SameSite::Lax)
                .build();

            (
                jar.add(cookie),
                Json(ApiResponse::ok(LoginRsp {
                    token,
                    requires_password_change: account.must_change_password,
                    password_expiry_days: None,
                })),
            )
                .into_response()
        }
        Err(_) => Json(ApiResponse::<()>::err(-1, "Token generation failed")).into_response(),
    }
}

pub async fn logout_handler(jar: CookieJar) -> impl IntoResponse {
    let cookie = Cookie::build((COOKIE_NAME, ""))
        .path("/")
        .max_age(Duration::ZERO.try_into().unwrap())
        .same_site(SameSite::Lax)
        .build();
    jar.add(cookie)
}

pub async fn change_password_handler(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<ChangePasswordReq>,
) -> impl IntoResponse {
    let mut account = get_account().await;

    let new_plain = decrypt_password(&req.password).unwrap_or(req.password.clone());

    if let Ok(hashed) = hash(new_plain, DEFAULT_COST) {
        account.username = req.username;
        account.password = hashed;
        account.must_change_password = false;
        account.last_password_change = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        );
        if save_account(&account).await.is_ok() {
            return Json(ApiResponse::ok(())).into_response();
        }
    }

    Json(ApiResponse::<()>::err(-1, "Failed to change password")).into_response()
}

pub async fn is_password_updated_handler() -> impl IntoResponse {
    let updated = Path::new(PWD_FILE).exists();
    Json(ApiResponse::ok(IsPasswordUpdatedRsp {
        is_updated: updated,
    }))
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
        Err(_) => Json(ApiResponse::<()>::err(-1, "Failed to get encryption key")).into_response(),
    }
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
        let secret = state.config.jwt.secret_key.as_bytes();
        let validation = Validation::new(Algorithm::HS256);
        if let Ok(data) = decode::<Claims>(&token, &DecodingKey::from_secret(secret), &validation) {
            if !data.claims.requires_password_change || req.uri().path().contains("/auth/password")
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

    let secret = state.config.jwt.secret_key.as_bytes();
    match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret),
    ) {
        Ok(token) => Ok(token),
        Err(_) => Err("Token generation failed".to_string()),
    }
}
