use crate::passkey::models::{RecoveryCode, RecoveryStorage, RECOVERY_CODES_FILE};
use chrono::Utc;
use rand::Rng;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;

const RATE_LIMIT_WINDOW_SECONDS: u64 = 900;
const MAX_ATTEMPTS_PER_WINDOW: u8 = 5;
const RATE_LIMIT_FILE: &str = "/etc/kvm/recovery_rate_limit";

static LAST_ATTEMPT_TIME: AtomicU64 = AtomicU64::new(0);
static ATTEMPT_COUNT: AtomicU8 = AtomicU8::new(0);

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
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(RECOVERY_CODES_FILE, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

pub async fn load_recovery_codes() -> std::io::Result<RecoveryStorage> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(metadata) = tokio::fs::metadata(RECOVERY_CODES_FILE).await {
            if let Ok(perms) = metadata.permissions().mode() {
                if perms & 0o777 != 0o600 {
                    tracing::warn!("Recovery codes file has incorrect permissions: {:o}", perms);
                }
            }
        }
    }

    match fs::read_to_string(RECOVERY_CODES_FILE).await {
        Ok(content) => serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)),
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

fn check_rate_limit() -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();
    let last_attempt = LAST_ATTEMPT_TIME.load(Ordering::SeqCst);
    let attempts = ATTEMPT_COUNT.load(Ordering::SeqCst);

    if now - last_attempt > RATE_LIMIT_WINDOW_SECONDS {
        LAST_ATTEMPT_TIME.store(now, Ordering::SeqCst);
        ATTEMPT_COUNT.store(1, Ordering::SeqCst);
        return true;
    }

    if attempts < MAX_ATTEMPTS_PER_WINDOW {
        ATTEMPT_COUNT.store(attempts + 1, Ordering::SeqCst);
        return true;
    }

    false
}

pub async fn validate_and_consume_code(input_code: &str) -> Result<(bool, u32), String> {
    if !check_rate_limit() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let last_attempt = LAST_ATTEMPT_TIME.load(Ordering::SeqCst);
        let wait_time = RATE_LIMIT_WINDOW_SECONDS - (now - last_attempt);
        return Err(format!(
            "Too many attempts. Please wait {} seconds.",
            wait_time
        ));
    }

    let normalized_input = input_code.trim().to_uppercase().replace("-", "");

    let mut storage = load_recovery_codes().await.map_err(|e| e.to_string())?;

    let mut found = false;
    let mut remaining = 0usize;

    for code in &storage.codes {
        if code.used {
            remaining += 1;
        } else {
            let normalized = code.code.replace("-", "");
            if normalized == normalized_input {
                found = true;
            } else {
                remaining += 1;
            }
        }
    }

    if found {
        for code in &mut storage.codes {
            let normalized = code.code.replace("-", "");
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
