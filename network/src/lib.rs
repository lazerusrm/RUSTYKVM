use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;
use tokio::fs;
use tokio::process::Command;
use tracing::{debug, error, info};

static MAC_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new("^[0-9A-F]{12}$").unwrap());

#[derive(Error, Debug)]
pub enum NetworkError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid MAC address: {0}")]
    InvalidMac(String),
    #[error("Command failed: {0}")]
    CommandFailed(String),
}

const WOL_MAC_FILE: &str = "/etc/kvm/cache/wol";
const WIFI_EXIST_FILE: &str = "/etc/kvm/wifi_exist";
const WIFI_AP_MODE_FILE: &str = "/tmp/wifiap";
const WIFI_SSID_FILE: &str = "/etc/kvm/wifi.ssid";
const WIFI_PASSWD_FILE: &str = "/etc/kvm/wifi.pass";
const WIFI_CONNECT_FILE: &str = "/kvmapp/kvm/wifi_try_connect";
const WIFI_STATE_FILE: &str = "/kvmapp/kvm/wifi_state";
const WIFI_SCRIPT: &str = "/etc/init.d/S30wifi";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WolEntry {
    pub mac: String,
    pub name: String,
}

pub struct NetworkManager;

impl NetworkManager {
    // --- Wake-on-LAN ---

    pub async fn wake_on_lan(mac: &str) -> Result<(), NetworkError> {
        let formatted_mac = Self::parse_mac(mac)?;

        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("ether-wake -b {}", formatted_mac))
            .output()
            .await?;

        if !output.status.success() {
            let err_msg = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(NetworkError::CommandFailed(err_msg));
        }

        let _ = Self::save_mac(&formatted_mac, "").await;

        info!("Wake-on-LAN sent to: {}", formatted_mac);
        Ok(())
    }

    pub async fn get_wol_macs() -> Result<Vec<WolEntry>, NetworkError> {
        if !Path::new(WOL_MAC_FILE).exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(WOL_MAC_FILE).await?;
        let mut entries = Vec::new();

        for line in content.lines() {
            let parts: Vec<&str> = line.splitn(2, ' ').collect();
            if !parts[0].is_empty() {
                entries.push(WolEntry {
                    mac: parts[0].to_string(),
                    name: parts.get(1).unwrap_or(&"").to_string(),
                });
            }
        }

        Ok(entries)
    }

    pub async fn set_wol_mac_name(mac: &str, name: &str) -> Result<(), NetworkError> {
        let mut entries = Self::get_wol_macs().await?;
        let mut found = false;

        for entry in &mut entries {
            if entry.mac == mac {
                entry.name = name.to_string();
                found = true;
                break;
            }
        }

        if !found {
            return Err(NetworkError::InvalidMac(format!(
                "MAC {} not found in cache",
                mac
            )));
        }

        Self::save_all_macs(&entries).await?;
        Ok(())
    }

    pub async fn delete_wol_mac(mac: &str) -> Result<(), NetworkError> {
        let mut entries = Self::get_wol_macs().await?;
        let original_len = entries.len();
        entries.retain(|e| e.mac != mac);

        if entries.len() != original_len {
            Self::save_all_macs(&entries).await?;
        }

        Ok(())
    }

    fn parse_mac(mac: &str) -> Result<String, NetworkError> {
        let clean_mac = mac
            .to_uppercase()
            .replace(['-', ':', '.'], "");

        if !MAC_REGEX.is_match(&clean_mac) {
            return Err(NetworkError::InvalidMac(mac.to_string()));
        }

        let mut result = String::new();
        for i in (0..12).step_by(2) {
            if i > 0 {
                result.push(':');
            }
            result.push_str(&clean_mac[i..i + 2]);
        }

        Ok(result)
    }

    fn sanitize_name(name: &str) -> String {
        name.chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
            .collect()
    }

    async fn save_mac(mac: &str, name: &str) -> Result<(), NetworkError> {
        let entries = Self::get_wol_macs().await?;
        if entries.iter().any(|e| e.mac == mac) {
            return Ok(());
        }

        if let Some(parent) = Path::new(WOL_MAC_FILE).parent() {
            fs::create_dir_all(parent).await?;
        }

        let sanitized_name = Self::sanitize_name(name);
        let line = if sanitized_name.is_empty() {
            format!("{}\n", mac)
        } else {
            format!("{} {}\n", mac, sanitized_name)
        };

        let mut file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(WOL_MAC_FILE)
            .await?;

        use tokio::io::AsyncWriteExt;
        file.write_all(line.as_bytes()).await?;
        Ok(())
    }

    async fn save_all_macs(entries: &[WolEntry]) -> Result<(), NetworkError> {
        let mut content = String::new();
        for entry in entries {
            if entry.name.is_empty() {
                content.push_str(&format!("{}\n", entry.mac));
            } else {
                content.push_str(&format!("{} {}\n", entry.mac, entry.name));
            }
        }
        fs::write(WOL_MAC_FILE, content).await?;
        Ok(())
    }

    // --- Wi-Fi ---

    pub async fn is_wifi_supported() -> bool {
        Path::new(WIFI_EXIST_FILE).exists()
    }

    pub async fn is_wifi_ap_mode() -> bool {
        Path::new(WIFI_AP_MODE_FILE).exists()
    }

    pub async fn is_wifi_connected() -> bool {
        match fs::read_to_string(WIFI_STATE_FILE).await {
            Ok(content) => content.trim() == "1",
            Err(_) => false,
        }
    }

    pub async fn get_wifi_ssid() -> Option<String> {
        match fs::read_to_string(WIFI_SSID_FILE).await {
            Ok(content) => Some(content.trim().to_string()),
            Err(_) => None,
        }
    }

    pub async fn connect_wifi(ssid: &str, password: &str) -> Result<(), NetworkError> {
        fs::write(WIFI_SSID_FILE, ssid).await?;
        fs::write(WIFI_PASSWD_FILE, password).await?;
        fs::write(WIFI_CONNECT_FILE, b"").await?;
        info!("WiFi connection initiated for SSID: {}", ssid);

        // Wait for connection to be established (up to 30 seconds)
        for i in 0..30 {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if Self::is_wifi_connected().await {
                info!(
                    "WiFi connected successfully to SSID: {} after {}s",
                    ssid,
                    i + 1
                );
                return Ok(());
            }
            debug!("Waiting for WiFi connection... ({}s)", i + 1);
        }

        error!("WiFi connection timed out for SSID: {}", ssid);
        Err(NetworkError::CommandFailed(
            "Connection timed out".to_string(),
        ))
    }

    pub async fn disconnect_wifi() -> Result<(), NetworkError> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!("{} stop", WIFI_SCRIPT))
            .output()
            .await?;

        if !output.status.success() {
            let err_msg = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(NetworkError::CommandFailed(err_msg));
        }

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;

        let _ = fs::remove_file(WIFI_SSID_FILE).await;
        let _ = fs::remove_file(WIFI_PASSWD_FILE).await;

        info!("WiFi disconnected");
        Ok(())
    }
}
