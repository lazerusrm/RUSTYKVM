use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;
use tracing::{info, warn};
use uuid::Uuid;
use base64::Engine;

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

fn default_proto() -> String { "https".to_string() }
fn default_auth() -> String { "enable".to_string() }
fn default_stun() -> String { "stun.l.google.com:19302".to_string() }

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
            level: default_log_level(),
            file: default_log_file(),
        }
    }
}

fn default_log_level() -> String { "info".to_string() }
fn default_log_file() -> String { "stdout".to_string() }

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
            http: default_http_port(),
            https: default_https_port(),
        }
    }
}

fn default_http_port() -> u16 { 80 }
fn default_https_port() -> u16 { 443 }

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
            crt: default_cert(),
            key: default_key(),
        }
    }
}

fn default_cert() -> String { "server.crt".to_string() }
fn default_key() -> String { "server.key".to_string() }

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
            refresh_token_duration: default_jwt_duration(),
            revoke_tokens_on_logout: default_true(),
        }
    }
}

fn generate_random_secret() -> String {
    use ring::rand::SecureRandom;

    let rng = ring::rand::SystemRandom::new();
    let mut key = [0u8; 32];
    rng.fill(&mut key).expect("Failed to generate random bytes");
    base64::engine::general_purpose::STANDARD.encode(&key)
}

fn default_jwt_duration() -> u64 { 2678400 }
fn default_true() -> bool { true }

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
    #[serde(default = "default_min_length")]
    pub min_length: u8,
    #[serde(default = "default_max_length")]
    pub max_length: u8,
    #[serde(default = "default_require_uppercase")]
    pub require_uppercase: bool,
    #[serde(default = "default_require_lowercase")]
    pub require_lowercase: bool,
    #[serde(default = "default_require_digit")]
    pub require_digit: bool,
    #[serde(default = "default_require_special")]
    pub require_special: bool,
    #[serde(default = "default_special_chars")]
    pub special_chars: String,
    #[serde(default = "default_max_age_days")]
    pub max_age_days: u16,
    #[serde(default = "default_min_age_days")]
    pub min_age_days: u16,
    #[serde(default = "default_history_count")]
    pub history_count: u8,
    #[serde(default = "default_lockout_threshold")]
    pub lockout_threshold: u8,
    #[serde(default = "default_lockout_duration_minutes")]
    pub lockout_duration_minutes: u16,
    #[serde(default = "default_force_first_change")]
    pub force_first_password_change: bool,
}

fn default_min_length() -> u8 { 8 }
fn default_max_length() -> u8 { 128 }
fn default_require_uppercase() -> bool { true }
fn default_require_lowercase() -> bool { true }
fn default_require_digit() -> bool { true }
fn default_require_special() -> bool { true }
fn default_special_chars() -> String { "!@#$%^&*()_+-=[]{}|;:,.<>?".to_string() }
fn default_max_age_days() -> u16 { 90 }
fn default_min_age_days() -> u16 { 1 }
fn default_history_count() -> u8 { 12 }
fn default_lockout_threshold() -> u8 { 5 }
fn default_lockout_duration_minutes() -> u16 { 30 }
fn default_force_first_change() -> bool { true }

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            min_length: default_min_length(),
            max_length: default_max_length(),
            require_uppercase: default_require_uppercase(),
            require_lowercase: default_require_lowercase(),
            require_digit: default_require_digit(),
            require_special: default_require_special(),
            special_chars: default_special_chars(),
            max_age_days: default_max_age_days(),
            min_age_days: default_min_age_days(),
            history_count: default_history_count(),
            lockout_threshold: default_lockout_threshold(),
            lockout_duration_minutes: default_lockout_duration_minutes(),
            force_first_password_change: default_force_first_change(),
        }
    }
}

const CONFIG_FILE: &str = "/etc/kvm/config.yaml";

impl Config {
    pub async fn load() -> Self {
        let path = Path::new(CONFIG_FILE);
        
        if path.exists() {
            match fs::read_to_string(path).await {
                Ok(content) => {
                    match serde_yaml::from_str::<Config>(&content) {
                        Ok(config) => {
                            info!("Configuration loaded from {}", CONFIG_FILE);
                            return config;
                        }
                        Err(e) => {
                            warn!("Failed to parse config file: {}. Using defaults.", e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read config file: {}. Using defaults.", e);
                }
            }
        } else {
            info!("Config file not found at {}. Using defaults.", CONFIG_FILE);
        }

        let default_config = Config::default();
        // If it doesn't exist, we might want to save the defaults (especially the generated secret)
        // But for now, let's just return it.
        default_config
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
            proto: default_proto(),
            port: Port::default(),
            cert: Cert::default(),
            logger: Logger::default(),
            authentication: default_auth(),
            jwt: JwtConfig::default(),
            stun: default_stun(),
            turn: Turn::default(),
            password_policy: PasswordPolicy::default(),
        }
    }
}
