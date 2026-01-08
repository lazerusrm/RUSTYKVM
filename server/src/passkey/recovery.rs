use std::fs;
use rand::Rng;
use chrono::Utc;
use crate::passkey::models::{RecoveryStorage, RecoveryCode, RECOVERY_CODES_FILE};

pub fn generate_recovery_code() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    let code: String = (0..16)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();
    // Format as XXXX-XXXX-XXXX-XXXX
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
        fs::create_dir_all(parent).ok();
    }
    let json = serde_json::to_string_pretty(storage)?;
    fs::write(RECOVERY_CODES_FILE, json)?;
    // Set restrictive permissions
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(RECOVERY_CODES_FILE, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub async fn load_recovery_codes() -> std::io::Result<RecoveryStorage> {
    match fs::read_to_string(RECOVERY_CODES_FILE).await {
        Ok(content) => {
            serde_json::from_str(&content).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        }
        Err(e) => Err(e),
    }
}

pub async fn validate_and_consume_code(input_code: &str) -> Result<(bool, u32), String> {
    let normalized_input = input_code.trim().to_uppercase().replace("-", "");
    
    let mut storage = load_recovery_codes().await.map_err(|e| e.to_string())?;
    
    let mut found = false;
    let mut used_any = false;
    let mut remaining = 0usize;
    
    for code in &storage.codes {
        if code.used {
            remaining += 1;
        } else {
            let normalized = code.code.replace("-", "");
            if normalized == normalized_input {
                found = true;
                // Don't mark as used yet - caller will save
            } else {
                remaining += 1;
            }
        }
    }
    
    if found {
        // Mark as used
        for code in &mut storage.codes {
            let normalized = code.code.replace("-", "");
            if normalized == normalized_input && !code.used {
                code.used = true;
                used_any = true;
                break;
            }
        }
        save_recovery_codes(&storage).await.map_err(|e| e.to_string())?;
        Ok((true, remaining as u32))
    } else {
        Ok((false, remaining as u32))
    }
}

pub async fn get_remaining_codes_count() -> u32 {
    match load_recovery_codes().await {
        Ok(storage) => storage.codes.iter().filter(|c| !c.used).count() as u32,
        Err(_) => 0,
    }
}

pub fn format_recovery_codes_for_display(storage: &RecoveryStorage) -> Vec<String> {
    storage.codes
        .iter()
        .filter(|c| !c.used)
        .map(|c| c.code.clone())
        .collect()
}
