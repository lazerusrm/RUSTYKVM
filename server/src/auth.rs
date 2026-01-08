use axum::{
    extract::{State, Json},
    response::{IntoResponse, Response},
    middleware::Next,
    http::{Request, StatusCode, header},
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use jsonwebtoken::{encode, decode, Header, Algorithm, Validation, EncodingKey, DecodingKey};
use chrono::{Utc, Duration, DateTime};
use bcrypt::{verify, hash, DEFAULT_COST};
use tracing::{info, warn, error, debug, event};
use crate::AppState;
use crate::utils::decrypt_password;
use std::path::Path;
use tokio::fs;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use once_cell::sync::Lazy;
use regex::Regex;

const COOKIE_NAME: &str = "nano-kvm-token";
const PWD_FILE: &str = "/etc/kvm/pwd";
const PWD_HISTORY_FILE: &str = "/etc/kvm/pwd_history";
const AUDIT_LOG_FILE: &str = "/var/log/nanokvm_auth.log";
const LOCKOUT_FILE: &str = "/etc/kvm/lockout";

static PASSWORD_REGEX: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?=.*[a-z])(?=.*[A-Z])(?=.*\d)(?=.*[!@#$%^&*()_+\-=\[\]{}|;:,.<>?]).{8,}$").unwrap()
});

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
pub struct PasswordHistory {
    pub entries: Vec<PasswordEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PasswordEntry {
    pub hash: String,
    pub timestamp: u64,
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
    pub old_password: String,
    pub new_password: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PasswordValidationRsp {
    pub valid: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PolicyRsp {
    pub min_length: u8,
    pub require_uppercase: bool,
    pub require_lowercase: bool,
    pub require_digit: bool,
    pub require_special: bool,
    pub max_age_days: u16,
    pub force_first_change: bool,
    pub locked: bool,
    pub locked_until: Option<String>,
}

static FAILED_ATTEMPTS: AtomicU8 = AtomicU8::new(0);
static LAST_LOCKOUT: AtomicU64 = AtomicU64::new(0);

async fn get_account() -> Account {
    match fs::read_to_string(PWD_FILE).await {
        Ok(content) => {
            if let Ok(account) = serde_json::from_str::<Account>(&content) {
                return account;
            }
        }
        Err(_) => {}
    }

    let hashed_password = match hash("admin", DEFAULT_COST) {
        Ok(h) => h,
        Err(e) => {
            error!("Failed to hash default password: {}. Using pre-computed fallback.", e);
            "$2b$12$LQv3c1yqBWVHxkd0LHAkCOYz6TtxMQJqhN8/X4.nY.yx7xLw0t7i".to_string()
        }
    };

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

async fn get_password_history() -> Vec<String> {
    match fs::read_to_string(PWD_HISTORY_FILE).await {
        Ok(content) => {
            if let Ok(history) = serde_json::from_str::<PasswordHistory>(&content) {
                return history.entries.into_iter().map(|e| e.hash).collect();
            }
        }
        Err(_) => {}
    }
    Vec::new()
}

async fn add_to_password_history(hash: &str) -> anyhow::Result<()> {
    let mut history = get_password_history().await;
    history.insert(0, hash.to_string());

    let max_history = 12;
    if history.len() > max_history {
        history.truncate(max_history);
    }

    let history_content = PasswordHistory {
        entries: history.iter().map(|h| PasswordEntry {
            hash: h.clone(),
            timestamp: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        }).collect(),
    };

    let content = serde_json::to_string(&history_content)?;
    fs::write(PWD_HISTORY_FILE, content).await?;
    Ok(())
}

fn check_lockout(account: &Account) -> bool {
    if let Some(locked_until) = account.locked_until {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        if now < locked_until {
            return true;
        }
    }
    false
}

fn get_lockout_duration_minutes(state: &Arc<AppState>) -> u16 {
    state.config.password_policy.lockout_duration_minutes
}

fn get_lockout_threshold(state: &Arc<AppState>) -> u8 {
    state.config.password_policy.lockout_threshold
}

async fn record_failed_attempt(account: &mut Account, state: &Arc<AppState>) {
    account.failed_attempts += 1;
    let threshold = get_lockout_threshold(state);

    if account.failed_attempts >= threshold {
        let lockout_duration = get_lockout_duration_minutes(state) as u64 * 60;
        let locked_until = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + lockout_duration;
        account.locked_until = Some(locked_until);
        account.failed_attempts = 0;

        let _ = save_account(account).await;
        event!(tracing::Level::WARN, "Account locked due to failed attempts");
    } else {
        let _ = save_account(account).await;
    }
}

async fn record_success(account: &mut Account) {
    account.failed_attempts = 0;
    account.locked_until = None;
    let _ = save_account(account).await;
}

async fn log_audit_event(
    event_type: &str,
    username: &str,
    success: bool,
    details: &str,
    ip_address: Option<&str>,
) {
    let entry = AuditLogEntry {
        timestamp: Utc::now().to_rfc3339(),
        event_type: event_type.to_string(),
        username: username.to_string(),
        ip_address: ip_address.map(|s| s.to_string()),
        success,
        details: details.to_string(),
    };

    let content = match serde_json::to_string(&entry) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut log_entry = content.clone();
    log_entry.push('\n');

    let _ = fs::write(AUDIT_LOG_FILE, log_entry).await;
}

pub fn validate_password(password: &str, state: &Arc<AppState>) -> Vec<String> {
    let policy = &state.config.password_policy;
    let mut errors = Vec::new();

    if password.len() < policy.min_length as usize {
        errors.push(format!("Password must be at least {} characters", policy.min_length));
    }

    if password.len() > policy.max_length as usize {
        errors.push(format!("Password must not exceed {} characters", policy.max_length));
    }

    if policy.require_uppercase && !password.chars().any(|c| c.is_uppercase()) {
        errors.push("Password must contain at least one uppercase letter".to_string());
    }

    if policy.require_lowercase && !password.chars().any(|c| c.is_lowercase()) {
        errors.push("Password must contain at least one lowercase letter".to_string());
    }

    if policy.require_digit && !password.chars().any(|c| c.is_ascii_digit()) {
        errors.push("Password must contain at least one digit".to_string());
    }

    if policy.require_special {
        let has_special = policy.special_chars.chars().any(|c| password.contains(c));
        if !has_special {
            errors.push(format!("Password must contain at least one special character from: {}", policy.special_chars));
        }
    }

    errors
}

async fn is_password_in_history(new_password: &str, state: &Arc<AppState>) -> bool {
    let history = get_password_history().await;
    let policy = &state.config.password_policy;
    let history_count = policy.history_count as usize;

    for old_hash in history.iter().take(history_count) {
        if verify(new_password, old_hash).unwrap_or(false) {
            return true;
        }
    }
    false
}

async fn get_session_ip(req: &Request<axum::body::Body>) -> Option<String> {
    req.headers()
        .get(header::X_FORWARDED_FOR)
        .or(req.headers().get(header::X_REAL_IP))
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordReq {
    pub old: String,
    pub new: String,
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

pub async fn is_password_updated_handler() -> impl IntoResponse {
    let updated = std::path::Path::new("/etc/kvm/pwd").exists();
    Json(IsPasswordUpdatedRsp { updated })
}

pub async fn get_account_handler() -> impl IntoResponse {
    let account = get_account().await;
    Json(GetAccountRsp { username: account.username })
}

pub async fn change_password_handler(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<ChangePasswordReq>,
) -> impl IntoResponse {
    let account = get_account().await;
    
    let old_plain = decrypt_password(&req.old).unwrap_or(req.old.clone());
    let new_plain = decrypt_password(&req.new).unwrap_or(req.new.clone());

    if !verify(&old_plain, &account.password).unwrap_or(false) {
        return (StatusCode::BAD_REQUEST, "Invalid old password").into_response();
    }

    if let Ok(hashed) = hash(new_plain, DEFAULT_COST) {
        let new_account = Account { username: account.username, password: hashed };
        if let Ok(json) = serde_json::to_string(&new_account) {
            if tokio::fs::write("/etc/kvm/pwd", json).await.is_ok() {
                return StatusCode::OK.into_response();
            }
        }
    }

    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

pub async fn login_handler(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<LoginReq>,
    req_http: Request<axum::body::Body>,
) -> impl IntoResponse {
    let ip_address = get_session_ip(&req_http).await;

    info!("Login attempt for user: {} from IP: {:?}", req.username, ip_address);

    if state.config.authentication == "disable" {
        return (jar, Json(LoginRsp { token: "disabled".to_string(), requires_password_change: false, password_expiry_days: None })).into_response();
    }

    let mut account = get_account().await;

    if check_lockout(&account) {
        let locked_until = account.locked_until.unwrap();
        let until_str = DateTime::from_timestamp(locked_until as i64, 0)
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| "unknown".to_string());

        log_audit_event("LOGIN_LOCKED", &req.username, false, "Account is locked", ip_address.as_deref()).await;

        return (StatusCode::FORBIDDEN, "Account is locked. Try again later.").into_response();
    }

    let plain_password = match decrypt_password(&req.password) {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to decrypt password: {}", e);
            req.password.clone()
        }
    };

    let password_valid = match verify(&plain_password, &account.password) {
        Ok(valid) => valid,
        Err(e) => {
            error!("Password verification error: {}", e);
            false
        }
    };

    if !password_valid || req.username != account.username {
        warn!("Login failed for user: {} from IP: {:?}", req.username, ip_address);
        record_failed_attempt(&mut account, &state).await;
        log_audit_event("LOGIN_FAILED", &req.username, false, "Invalid credentials", ip_address.as_deref()).await;
        return (StatusCode::UNAUTHORIZED, "Invalid username or password").into_response();
    }

    record_success(&mut account).await;

    let session_id = uuid::Uuid::new_v4().to_string();

    let mut requires_change = account.must_change_password;

    let mut password_expiry_days: Option<i64> = None;
    if let Some(last_change) = account.last_password_change {
        let max_age_seconds = state.config.password_policy.max_age_days as i64 * 24 * 60 * 60;
        let expires_at = last_change as i64 + max_age_seconds;
        let now = Utc::now().timestamp();

        if expires_at <= now {
            requires_change = true;
        } else {
            password_expiry_days = Some(((expires_at - now) / (24 * 60 * 60)) as i64);
        }
    }

    let exp = Utc::now() + Duration::seconds(state.config.jwt.refresh_token_duration as i64);
    let claims = Claims {
        username: req.username,
        exp: exp.timestamp() as usize,
        requires_password_change: requires_change,
        session_id: session_id.clone(),
    };

    let secret = state.config.jwt.secret_key.as_bytes();
    match encode(&Header::default(), &claims, &EncodingKey::from_secret(secret)) {
        Ok(token) => {
            let cookie = Cookie::build((COOKIE_NAME, token.clone()))
                .path("/")
                .http_only(true)
                .finish();

            log_audit_event("LOGIN_SUCCESS", &req.username, true, "Successful login", ip_address.as_deref()).await;

            (jar.add(cookie), Json(LoginRsp { token, requires_password_change, password_expiry_days })).into_response()
        }
        Err(e) => {
            error!("JWT encoding error: {}", e);
            log_audit_event("LOGIN_ERROR", &req.username, false, "JWT encoding failed", ip_address.as_deref()).await;
            (StatusCode::INTERNAL_SERVER_ERROR, "Token generation failed").into_response()
        }
    }
}

pub async fn logout_handler(jar: CookieJar, req: Request<axum::body::Body>) -> impl IntoResponse {
    let ip_address = get_session_ip(&req).await;

    let cookie = Cookie::build((COOKIE_NAME, ""))
        .path("/")
        .max_age(chrono::Duration::zero().to_std().unwrap().into())
        .finish();

    log_audit_event("LOGOUT", "user", true, "User logged out", ip_address.as_deref()).await;

    jar.add(cookie)
}

pub async fn change_password_handler(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(req): Json<ChangePasswordReq>,
    req_http: Request<axum::body::Body>,
) -> impl IntoResponse {
    let ip_address = get_session_ip(&req_http).await;

    let mut account = get_account().await;

    let plain_old_password = match decrypt_password(&req.old_password) {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to decrypt old password: {}", e);
            req.old_password.clone()
        }
    };

    if !verify(&plain_old_password, &account.password).unwrap_or(false) {
        log_audit_event("PASSWORD_CHANGE", &account.username, false, "Old password incorrect", ip_address.as_deref()).await;
        return (StatusCode::UNAUTHORIZED, "Current password is incorrect").into_response();
    }

    let validation_errors = validate_password(&req.new_password, &state);
    if !validation_errors.is_empty() {
        log_audit_event("PASSWORD_CHANGE", &account.username, false, "New password doesn't meet policy", ip_address.as_deref()).await;
        return (StatusCode::BAD_REQUEST, format!("Password doesn't meet requirements: {}", validation_errors.join(", "))).into_response();
    }

    if is_password_in_history(&req.new_password, &state).await {
        log_audit_event("PASSWORD_CHANGE", &account.username, false, "Password matches history", ip_address.as_deref()).await;
        return (StatusCode::BAD_REQUEST, "Password cannot match any of your previous passwords").into_response();
    }

    let plain_new_password = match decrypt_password(&req.new_password) {
        Ok(p) => p,
        Err(_) => req.new_password.clone(),
    };

    let new_hash = match hash(&plain_new_password, DEFAULT_COST) {
        Ok(h) => h,
        Err(e) => {
            error!("Failed to hash new password: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to process new password").into_response();
        }
    };

    add_to_password_history(&account.password).await;

    account.password = new_hash;
    account.last_password_change = Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs());
    account.must_change_password = false;

    let _ = save_account(&account).await;

    let exp = Utc::now() + Duration::seconds(state.config.jwt.refresh_token_duration as i64);
    let claims = Claims {
        username: account.username.clone(),
        exp: exp.timestamp() as usize,
        requires_password_change: false,
        session_id: uuid::Uuid::new_v4().to_string(),
    };

    let secret = state.config.jwt.secret_key.as_bytes();
    match encode(&Header::default(), &claims, &EncodingKey::from_secret(secret)) {
        Ok(token) => {
            let cookie = Cookie::build((COOKIE_NAME, token.clone()))
                .path("/")
                .http_only(true)
                .finish();

            log_audit_event("PASSWORD_CHANGE_SUCCESS", &account.username, true, "Password changed successfully", ip_address.as_deref()).await;

            (jar.add(cookie), Json(LoginRsp { token, requires_password_change: false, password_expiry_days: None })).into_response()
        }
        Err(e) => {
            error!("JWT encoding error after password change: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Token regeneration failed").into_response()
        }
    }
}

pub async fn get_password_policy_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let account = get_account().await;
    let locked = check_lockout(&account);

    let locked_until = if locked {
        account.locked_until.map(|ts| {
            DateTime::from_timestamp(ts as i64, 0)
                .map(|d| d.to_rfc3339())
                .unwrap_or_else(|| "unknown".to_string())
        })
    } else {
        None
    };

    Json(PolicyRsp {
        min_length: state.config.password_policy.min_length,
        require_uppercase: state.config.password_policy.require_uppercase,
        require_lowercase: state.config.password_policy.require_lowercase,
        require_digit: state.config.password_policy.require_digit,
        require_special: state.config.password_policy.require_special,
        max_age_days: state.config.password_policy.max_age_days,
        force_first_change: state.config.password_policy.force_first_password_change,
        locked,
        locked_until,
    })
}

pub async fn validate_password_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChangePasswordReq>,
) -> impl IntoResponse {
    let plain_new_password = match decrypt_password(&req.new_password) {
        Ok(p) => p,
        Err(_) => req.new_password.clone(),
    };

    let errors = validate_password(&plain_new_password, &state);

    if !errors.is_empty() {
        return Json(PasswordValidationRsp { valid: false, errors });
    }

    if is_password_in_history(&plain_new_password, &state).await {
        return Json(PasswordValidationRsp {
            valid: false,
            errors: vec!["Password cannot match any of your previous passwords".to_string()],
        });
    }

    Json(PasswordValidationRsp { valid: true, errors: Vec::new() })
}

pub async fn auth_middleware<B>(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    req: Request<B>,
    next: Next<B>,
) -> Response {
    if state.config.authentication == "disable" {
        return next.run(req).await;
    }

    let token = jar.get(COOKIE_NAME)
        .map(|c| c.value().to_string())
        .or_else(|| {
            req.headers()
                .get(header::AUTHORIZATION)
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.strip_prefix("Bearer "))
                .map(|s| s.to_string())
        });

    if let Some(token) = token {
        let secret = state.config.jwt.secret_key.as_bytes();
        let validation = Validation::new(Algorithm::HS256);
        match decode::<Claims>(&token, &DecodingKey::from_secret(secret), &validation) {
            Ok(claims) => {
                if claims.requires_password_change {
                    let response = (StatusCode::FORBIDDEN, "Password change required").into_response();
                    return response;
                }
                return next.run(req).await;
            }
            Err(e) => {
                warn!("Invalid token: {}", e);
            }
        }
    }

    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}
