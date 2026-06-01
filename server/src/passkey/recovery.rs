#![allow(dead_code)]

use crate::passkey::models::{RecoveryCode, RecoveryStorage, RECOVERY_CODES_FILE};
use chrono::Utc;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::sync::{Mutex, OnceCell};

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;

const RATE_LIMIT_WINDOW_SECONDS: u64 = 900;
const MAX_ATTEMPTS_PER_WINDOW: u8 = 5;
const RATE_LIMIT_FILE: &str = "/etc/kvm/recovery_rate_limit";

#[derive(Serialize, Deserialize, Default, Clone)]
struct RateLimitStore {
    by_ip: HashMap<String, RateLimitState>,
}

#[derive(Serialize, Deserialize, Clone)]
struct RateLimitState {
    last_attempt: u64,
    attempts: u8,
}

static RATE_LIMITS: OnceLock<Mutex<RateLimitStore>> = OnceLock::new();
static RATE_LIMIT_LOADED: OnceCell<()> = OnceCell::const_new();

fn rate_limits() -> &'static Mutex<RateLimitStore> {
    RATE_LIMITS.get_or_init(|| Mutex::new(RateLimitStore::default()))
}

async fn load_rate_limit_state() {
    let _ = RATE_LIMIT_LOADED
        .get_or_init(|| async {
            if let Ok(content) = fs::read_to_string(RATE_LIMIT_FILE).await {
                if let Ok(store) = serde_json::from_str::<RateLimitStore>(&content) {
                    *rate_limits().lock().await = store;
                }
            }
        })
        .await;
}

async fn save_rate_limit_state() {
    let store = rate_limits().lock().await.clone();
    if let Ok(json) = serde_json::to_string(&store) {
        let _ = fs::write(RATE_LIMIT_FILE, json).await;
        #[cfg(target_os = "linux")]
        {
            let _ = fs::set_permissions(RATE_LIMIT_FILE, std::fs::Permissions::from_mode(0o600)).await;
        }
    }
}

async fn check_rate_limit(client_ip: &str) -> bool {
    load_rate_limit_state().await;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();

    let mut store = rate_limits().lock().await;
    let entry = store
        .by_ip
        .entry(client_ip.to_string())
        .or_insert(RateLimitState {
            last_attempt: 0,
            attempts: 0,
        });

    if now.saturating_sub(entry.last_attempt) > RATE_LIMIT_WINDOW_SECONDS {
        entry.last_attempt = now;
        entry.attempts = 1;
        drop(store);
        save_rate_limit_state().await;
        return true;
    }

    if entry.attempts < MAX_ATTEMPTS_PER_WINDOW {
        entry.attempts += 1;
        drop(store);
        save_rate_limit_state().await;
        return true;
    }

    false
}

pub fn generate_recovery_code() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    let code: String = (0..16)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();
    format!(
        "{}-{}-{}-{}",
        &code[0..4],
        &code[4..8],
        &code[8..12],
        &code[12..16]
    )
}

pub fn generate_recovery_codes(count: usize) -> RecoveryStorage {
    let codes: Vec<RecoveryCode> = (0..count)
        .map(|_| RecoveryCode {
            code: generate_recovery_code(),
            used: false,
        })
        .collect();

    RecoveryStorage {
        codes,
        created: Utc::now(),
    }
}

pub async fn save_recovery_codes(storage: &RecoveryStorage) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(RECOVERY_CODES_FILE).parent() {
        fs::create_dir_all(parent).await.ok();
    }
    let json = serde_json::to_string_pretty(storage)?;
    fs::write(RECOVERY_CODES_FILE, json).await?;
    #[cfg(target_os = "linux")]
    {
        fs::set_permissions(RECOVERY_CODES_FILE, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

pub async fn load_recovery_codes() -> std::io::Result<RecoveryStorage> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(metadata) = tokio::fs::metadata(RECOVERY_CODES_FILE).await {
            let perms = metadata.permissions();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if perms.mode() & 0o777 != 0o600 {
                    tracing::warn!(
                        "Recovery codes file has incorrect permissions: {:o}",
                        perms.mode()
                    );
                }
            }
        }
    }

    match fs::read_to_string(RECOVERY_CODES_FILE).await {
        Ok(content) => serde_json::from_str(&content).map_err(std::io::Error::other),
        Err(e) => Err(e),
    }
}

pub async fn get_remaining_codes_count() -> u32 {
    match load_recovery_codes().await {
        Ok(storage) => storage.codes.iter().filter(|c| !c.used).count() as u32,
        Err(_) => 0,
    }
}

pub fn format_recovery_codes_for_display(storage: &RecoveryStorage) -> Vec<String> {
    storage
        .codes
        .iter()
        .filter(|c| !c.used)
        .map(|c| c.code.clone())
        .collect()
}

pub async fn validate_and_consume_code(
    input_code: &str,
    client_ip: &str,
) -> Result<(bool, u32), String> {
    if !check_rate_limit(client_ip).await {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let store = rate_limits().lock().await;
        let wait_time = store
            .by_ip
            .get(client_ip)
            .map(|entry| RATE_LIMIT_WINDOW_SECONDS.saturating_sub(now.saturating_sub(entry.last_attempt)))
            .unwrap_or(0);
        return Err(format!(
            "Too many attempts. Please wait {} seconds.",
            wait_time
        ));
    }

    let normalized_input = input_code.trim().to_uppercase().replace('-', "");

    let mut storage = load_recovery_codes().await.map_err(|e| e.to_string())?;

    let mut found = false;
    let mut remaining = 0usize;

    for code in &storage.codes {
        if code.used {
            remaining += 1;
        } else {
            let normalized = code.code.replace('-', "");
            if normalized == normalized_input {
                found = true;
            } else {
                remaining += 1;
            }
        }
    }

    if found {
        for code in &mut storage.codes {
            let normalized = code.code.replace('-', "");
            if normalized == normalized_input && !code.used {
                code.used = true;
                break;
            }
        }
        save_recovery_codes(&storage)
            .await
            .map_err(|e| e.to_string())?;
        Ok((true, remaining as u32))
    } else {
        Ok((false, remaining as u32))
    }
}