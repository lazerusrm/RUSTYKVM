use aes::Aes256;
use cbc::Decryptor;
use cipher::{BlockDecryptMut, KeyIvInit};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::env;
use std::path::Path;
use std::fs;
use walkdir::WalkDir;

const SECRET_KEY_ENV: &str = "NANOKVM_SECRET_KEY";

fn get_secret_key() -> anyhow::Result<String> {
    env::var(SECRET_KEY_ENV).map_err(|_| {
        anyhow::anyhow!(
            "NANOKVM_SECRET_KEY environment variable is not set. \
            Please set this variable to a secure key before running the application."
        )
    })
}

pub fn decrypt_password(ciphertext: &str) -> anyhow::Result<String> {
    if ciphertext.is_empty() {
        return Ok(String::new());
    }

    let data = BASE64.decode(ciphertext)?;

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

fn derive_key_iv(password: &[u8], salt: &[u8], key_len: usize, iv_len: usize) -> (Vec<u8>, Vec<u8>) {
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