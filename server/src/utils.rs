#![allow(dead_code)]

use aes::Aes256;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cbc::Decryptor;
use cipher::{BlockDecryptMut, KeyIvInit};
use std::env;
use std::fs;
use std::path::Path;
#[cfg(target_os = "linux")]
use walkdir::WalkDir;

const SECRET_KEY_ENV: &str = "NANOKVM_SECRET_KEY";
const SECRET_KEY_FILE: &str = "/etc/kvm/secret_key";
const SECRET_KEY_LENGTH: usize = 32;

/// Get or generate the device-specific encryption key.
/// Priority: 1) Environment variable, 2) File, 3) Generate and save new key
pub fn get_secret_key() -> anyhow::Result<String> {
    // First check environment variable
    if let Ok(key) = env::var(SECRET_KEY_ENV) {
        if !key.is_empty() {
            return Ok(key);
        }
    }

    // Check if key file exists
    if let Ok(key) = fs::read_to_string(SECRET_KEY_FILE) {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(key);
        }
    }

    // Generate new key and save it
    let key = generate_secret_key();
    save_secret_key(&key)?;
    Ok(key)
}

/// Generate a cryptographically secure random key
fn generate_secret_key() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..SECRET_KEY_LENGTH)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

/// Save the secret key to file with restricted permissions
fn save_secret_key(key: &str) -> anyhow::Result<()> {
    // Ensure directory exists
    if let Some(parent) = Path::new(SECRET_KEY_FILE).parent() {
        fs::create_dir_all(parent)?;
    }

    // Write key to file
    fs::write(SECRET_KEY_FILE, key)?;

    // Set restrictive permissions (owner read/write only)
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(SECRET_KEY_FILE, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

pub fn decrypt_password(ciphertext: &str) -> anyhow::Result<String> {
    if ciphertext.is_empty() {
        return Ok(String::new());
    }

    // URL-decode first (frontend uses encodeURIComponent)
    let decoded_ciphertext = urlencoding::decode(ciphertext)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| ciphertext.to_string());

    let data = BASE64.decode(&decoded_ciphertext)?;

    if data.len() < 16 || &data[0..8] != b"Salted__" {
        return Err(anyhow::anyhow!("Invalid format"));
    }

    let salt = &data[8..16];
    let encrypted_data = &data[16..];

    let secret_key = get_secret_key()?;
    let (key, iv) = derive_key_iv(secret_key.as_bytes(), salt, 32, 16);

    type Aes256CbcDec = Decryptor<Aes256>;
    let mut buf = encrypted_data.to_vec();
    let pt = Aes256CbcDec::new_from_slices(&key, &iv)
        .map_err(|e| anyhow::anyhow!("Init error: {}", e))?
        .decrypt_padded_mut::<cipher::block_padding::Pkcs7>(&mut buf)
        .map_err(|e| anyhow::anyhow!("Decrypt error: {}", e))?;

    Ok(String::from_utf8_lossy(pt).to_string())
}

fn derive_key_iv(
    password: &[u8],
    salt: &[u8],
    key_len: usize,
    iv_len: usize,
) -> (Vec<u8>, Vec<u8>) {
    let mut derived_bytes = Vec::new();
    let mut last_digest: Vec<u8> = Vec::new();

    while derived_bytes.len() < key_len + iv_len {
        let mut data = Vec::new();
        data.extend_from_slice(&last_digest);
        data.extend_from_slice(password);
        data.extend_from_slice(salt);
        let digest = md5::compute(&data);
        last_digest = digest.to_vec();
        derived_bytes.extend_from_slice(&last_digest);
    }

    let key = derived_bytes[0..key_len].to_vec();
    let iv = derived_bytes[key_len..key_len + iv_len].to_vec();
    (key, iv)
}

pub fn ensure_permission(path: &str, mode: u32) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    let _ = (path, mode);
    Ok(())
}

pub fn chmod_recursively(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                fs::set_permissions(entry.path(), fs::Permissions::from_mode(mode))?;
            }
        }
    }
    let _ = (path, mode);
    Ok(())
}

pub fn unzip(src: &Path, dst: &Path) -> anyhow::Result<()> {
    let file = fs::File::open(src)?;
    let mut archive = zip::ZipArchive::new(file)?;
    archive.extract(dst)?;
    Ok(())
}
