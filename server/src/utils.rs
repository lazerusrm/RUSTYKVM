use aes::Aes256;
use cbc::Decryptor;
use cipher::{BlockDecryptMut, KeyIvInit};
use md5::Context;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::env;

const SECRET_KEY_ENV: &str = "NANOKVM_SECRET_KEY";
const DEFAULT_SECRET_KEY: &str = "nanokvm-sipeed-2024";

fn get_secret_key() -> String {
    env::var(SECRET_KEY_ENV).unwrap_or_else(|_| DEFAULT_SECRET_KEY.to_string())
}

/// Decrypts data encrypted by CryptoJS.AES.encrypt(data, SECRET_KEY)
pub fn decrypt_password(ciphertext: &str) -> anyhow::Result<String> {
    if ciphertext.is_empty() {
        return Ok(String::new());
    }

    // 1. Decode base64
    let data = BASE64.decode(ciphertext)?;
    
    if data.len() < 16 || &data[0..8] != b"Salted__" {
        return Err(anyhow::anyhow!("Invalid encryption format: missing Salted__ prefix"));
    }

    // 2. Extract salt
    let salt = &data[8..16];
    let encrypted_data = &data[16..];

    // 3. Derive Key and IV using OpenSSL EVP_BytesToKey (MD5 based)
    // For AES-256-CBC we need 32 bytes key and 16 bytes IV = 48 bytes total.
    let secret_key = get_secret_key();
    let (key, iv) = derive_key_iv(secret_key.as_bytes(), salt, 32, 16);

    // 4. Decrypt
    type Aes256CbcDec = Decryptor<Aes256>;
    let mut buf = encrypted_data.to_vec();
    let pt = Aes256CbcDec::new_from_slices(&key, &iv)
        .map_err(|e| anyhow::anyhow!("Invalid key/iv length: {}", e))?
        .decrypt_padded_mut::<cipher::block_padding::Pkcs7>(&mut buf)
        .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))?;

    Ok(String::from_utf8_lossy(pt).to_string())
}

fn derive_key_iv(password: &[u8], salt: &[u8], key_len: usize, iv_len: usize) -> (Vec<u8>, Vec<u8>) {
    let mut derived_bytes = Vec::new();
    let mut last_digest: Vec<u8> = Vec::new();

    while derived_bytes.len() < key_len + iv_len {
        let mut hasher = Context::new();
        if !last_digest.is_empty() {
            hasher.update(&last_digest);
        }
        hasher.update(password);
        hasher.update(salt);
        last_digest = hasher.finalize().to_vec();
        derived_bytes.extend_from_slice(&last_digest);
    }

    let key = derived_bytes[0..key_len].to_vec();
    let iv = derived_bytes[key_len..key_len + iv_len].to_vec();
    (key, iv)
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonResponse<T> {
    pub code: i32,
    pub msg: String,
    pub data: Option<T>,
}

impl<T> JsonResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: 0,
            msg: "success".to_string(),
            data: Some(data),
        }
    }

    pub fn success() -> Self {
        Self {
            code: 0,
            msg: "success".to_string(),
            data: None,
        }
    }

    pub fn error(code: i32, msg: &str) -> Self {
        Self {
            code,
            msg: msg.to_string(),
            data: None,
        }
    }
}

pub fn move_files_recursively(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    let mut options = fs_extra::dir::CopyOptions::new();
    options.content_only = true;
    options.overwrite = true;
    fs_extra::dir::move_dir(src, dst, &options)?;
    Ok(())
}

pub fn ensure_permission(path: &str, mode: u32) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

pub fn chmod_recursively(path: &std::path::Path, mode: u32) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        for entry in walkdir::WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(mode))?;
            }
        }
    }
    Ok(())
}

pub fn unzip(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    let file = std::fs::File::open(src)?;
    let mut archive = zip::ZipArchive::new(file)?;
    archive.extract(dst)?;
    Ok(())
}
