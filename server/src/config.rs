use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;
use tracing::{info, warn};

// Default values as constants for clarity and reuse
// NOTE: Default to HTTP for safety - HTTPS requires valid TLS certs to exist
const DEFAULT_PROTO: &str = "http";
const DEFAULT_AUTH: &str = "enable";
const DEFAULT_STUN: &str = "stun.l.google.com:19302";
const DEFAULT_LOG_LEVEL: &str = "info";
const DEFAULT_LOG_FILE: &str = "stdout";
const DEFAULT_HTTP_PORT: u16 = 80;
const DEFAULT_HTTPS_PORT: u16 = 443;
const DEFAULT_CERT_FILE: &str = "server.crt";
const DEFAULT_KEY_FILE: &str = "server.key";
const DEFAULT_JWT_DURATION: u64 = 2678400; // 31 days
const DEFAULT_PW_MIN_LENGTH: u8 = 8;
const DEFAULT_PW_MAX_LENGTH: u8 = 128;
const DEFAULT_PW_SPECIAL_CHARS: &str = "!@#$%^&*()_+-=[]{}|;:,.<>?";
const DEFAULT_PW_MAX_AGE_DAYS: u16 = 90;
const DEFAULT_PW_MIN_AGE_DAYS: u16 = 1;
const DEFAULT_PW_HISTORY_COUNT: u8 = 12;
const DEFAULT_LOCKOUT_THRESHOLD: u8 = 5;
const DEFAULT_LOCKOUT_DURATION_MIN: u16 = 30;
const CONFIG_FILE: &str = "/etc/kvm/config.yaml";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default = "default_proto")]
    pub proto: String,
    #[serde(default)]
    pub port: Port,
    #[serde(default)]
    pub cert: Cert,
    #[serde(default)]
    pub logger: Logger,
    #[serde(default = "default_auth")]
    pub authentication: String,
    #[serde(default)]
    pub jwt: JwtConfig,
    #[serde(default = "default_stun")]
    pub stun: String,
    #[serde(default)]
    pub turn: Turn,
    #[serde(default)]
    pub password_policy: PasswordPolicy,
}

fn default_proto() -> String {
    DEFAULT_PROTO.to_string()
}
fn default_auth() -> String {
    DEFAULT_AUTH.to_string()
}
fn default_stun() -> String {
    DEFAULT_STUN.to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Logger {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_file")]
    pub file: String,
}

impl Default for Logger {
    fn default() -> Self {
        Self {
            level: DEFAULT_LOG_LEVEL.to_string(),
            file: DEFAULT_LOG_FILE.to_string(),
        }
    }
}

fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_string()
}
fn default_log_file() -> String {
    DEFAULT_LOG_FILE.to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Port {
    #[serde(default = "default_http_port")]
    pub http: u16,
    #[serde(default = "default_https_port")]
    pub https: u16,
}

impl Default for Port {
    fn default() -> Self {
        Self {
            http: DEFAULT_HTTP_PORT,
            https: DEFAULT_HTTPS_PORT,
        }
    }
}

fn default_http_port() -> u16 {
    DEFAULT_HTTP_PORT
}
fn default_https_port() -> u16 {
    DEFAULT_HTTPS_PORT
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Cert {
    #[serde(default = "default_cert")]
    pub crt: String,
    #[serde(default = "default_key")]
    pub key: String,
}

impl Default for Cert {
    fn default() -> Self {
        Self {
            crt: DEFAULT_CERT_FILE.to_string(),
            key: DEFAULT_KEY_FILE.to_string(),
        }
    }
}

fn default_cert() -> String {
    DEFAULT_CERT_FILE.to_string()
}
fn default_key() -> String {
    DEFAULT_KEY_FILE.to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JwtConfig {
    #[serde(rename = "secretKey", default = "generate_random_secret")]
    pub secret_key: String,
    #[serde(rename = "refreshTokenDuration", default = "default_jwt_duration")]
    pub refresh_token_duration: u64,
    #[serde(rename = "revokeTokensOnLogout", default = "default_true")]
    pub revoke_tokens_on_logout: bool,
}

impl Default for JwtConfig {
    fn default() -> Self {
        Self {
            secret_key: generate_random_secret(),
            refresh_token_duration: DEFAULT_JWT_DURATION,
            revoke_tokens_on_logout: true,
        }
    }
}

fn generate_random_secret() -> String {
    use ring::rand::SecureRandom;
    let rng = ring::rand::SystemRandom::new();
    let mut key = [0u8; 32];
    rng.fill(&mut key).expect("RNG failure");
    base64::engine::general_purpose::STANDARD.encode(key)
}

fn default_jwt_duration() -> u64 {
    DEFAULT_JWT_DURATION
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Turn {
    #[serde(rename = "turnAddr", default)]
    pub turn_addr: String,
    #[serde(rename = "turnUser", default)]
    pub turn_user: String,
    #[serde(rename = "turnCred", default)]
    pub turn_cred: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PasswordPolicy {
    #[serde(default = "default_pw_min")]
    pub min_length: u8,
    #[serde(default = "default_pw_max")]
    pub max_length: u8,
    #[serde(default = "default_true")]
    pub require_uppercase: bool,
    #[serde(default = "default_true")]
    pub require_lowercase: bool,
    #[serde(default = "default_true")]
    pub require_digit: bool,
    #[serde(default = "default_true")]
    pub require_special: bool,
    #[serde(default = "default_special_chars")]
    pub special_chars: String,
    #[serde(default = "default_max_age")]
    pub max_age_days: u16,
    #[serde(default = "default_min_age")]
    pub min_age_days: u16,
    #[serde(default = "default_history")]
    pub history_count: u8,
    #[serde(default = "default_lockout")]
    pub lockout_threshold: u8,
    #[serde(default = "default_lockout_duration")]
    pub lockout_duration_minutes: u16,
    #[serde(default = "default_true")]
    pub force_first_password_change: bool,
}

fn default_pw_min() -> u8 {
    DEFAULT_PW_MIN_LENGTH
}
fn default_pw_max() -> u8 {
    DEFAULT_PW_MAX_LENGTH
}
fn default_special_chars() -> String {
    DEFAULT_PW_SPECIAL_CHARS.to_string()
}
fn default_max_age() -> u16 {
    DEFAULT_PW_MAX_AGE_DAYS
}
fn default_min_age() -> u16 {
    DEFAULT_PW_MIN_AGE_DAYS
}
fn default_history() -> u8 {
    DEFAULT_PW_HISTORY_COUNT
}
fn default_lockout() -> u8 {
    DEFAULT_LOCKOUT_THRESHOLD
}
fn default_lockout_duration() -> u16 {
    DEFAULT_LOCKOUT_DURATION_MIN
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            min_length: DEFAULT_PW_MIN_LENGTH,
            max_length: DEFAULT_PW_MAX_LENGTH,
            require_uppercase: true,
            require_lowercase: true,
            require_digit: true,
            require_special: true,
            special_chars: DEFAULT_PW_SPECIAL_CHARS.to_string(),
            max_age_days: DEFAULT_PW_MAX_AGE_DAYS,
            min_age_days: DEFAULT_PW_MIN_AGE_DAYS,
            history_count: DEFAULT_PW_HISTORY_COUNT,
            lockout_threshold: DEFAULT_LOCKOUT_THRESHOLD,
            lockout_duration_minutes: DEFAULT_LOCKOUT_DURATION_MIN,
            force_first_password_change: true,
        }
    }
}

impl Config {
    pub async fn load() -> Self {
        let path = Path::new(CONFIG_FILE);

        if path.exists() {
            match fs::read_to_string(path).await {
                Ok(content) => match serde_yaml::from_str::<Config>(&content) {
                    Ok(config) => {
                        info!("Configuration loaded from {}", CONFIG_FILE);
                        return config;
                    }
                    Err(e) => {
                        warn!("Failed to parse config: {}. Using defaults.", e);
                    }
                },
                Err(e) => {
                    warn!("Failed to read config: {}. Using defaults.", e);
                }
            }
        } else {
            info!("Config not found at {}. Using defaults.", CONFIG_FILE);
        }

        Config::default()
    }

    pub async fn save(&self) -> anyhow::Result<()> {
        let content = serde_yaml::to_string(self)?;
        if let Some(parent) = Path::new(CONFIG_FILE).parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(CONFIG_FILE, content).await?;
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            proto: DEFAULT_PROTO.to_string(),
            port: Port::default(),
            cert: Cert::default(),
            logger: Logger::default(),
            authentication: DEFAULT_AUTH.to_string(),
            jwt: JwtConfig::default(),
            stun: DEFAULT_STUN.to_string(),
            turn: Turn::default(),
            password_policy: PasswordPolicy::default(),
        }
    }
}
