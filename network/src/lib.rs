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

// Ethernet constants
const ETH_NODHCP_FILE: &str = "/boot/eth.nodhcp";
const RESOLV_CONF_FILE: &str = "/etc/resolv.conf";
const ETH_CONFIG_FILE: &str = "/etc/kvm/ethernet.yaml";
const ETH_INIT_SCRIPT: &str = "/etc/init.d/S30eth";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WolEntry {
    pub mac: String,
    pub name: String,
}

/// Ethernet configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EthernetConfig {
    pub dhcp: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub netmask: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns2: Option<String>,
}

/// Response for GET ethernet config
#[derive(Debug, Serialize)]
pub struct GetEthernetConfigRsp {
    pub config: EthernetConfig,
    pub current: EthernetConfig,
}

/// Request for SET ethernet config
#[derive(Debug, Deserialize)]
pub struct SetEthernetConfigReq {
    pub dhcp: bool,
    pub ip: Option<String>,
    pub netmask: Option<String>,
    pub gateway: Option<String>,
    pub dns1: Option<String>,
    pub dns2: Option<String>,
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
        let clean_mac = mac.to_uppercase().replace(['-', ':', '.'], "");

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

    // --- Ethernet Configuration ---

    fn is_valid_ip(ip: &str) -> bool {
        ip.parse::<std::net::IpAddr>().is_ok()
    }

    fn is_valid_netmask(netmask: &str) -> bool {
        let parts: Vec<&str> = netmask.split('.').collect();
        if parts.len() != 4 {
            return false;
        }

        let octets: Result<Vec<u8>, _> = parts.iter().map(|p| p.parse::<u8>()).collect();
        let octets = match octets {
            Ok(o) => o,
            Err(_) => return false,
        };

        let bits = (octets[0] as u32) << 24
            | (octets[1] as u32) << 16
            | (octets[2] as u32) << 8
            | octets[3] as u32;
        let inverted = !bits;
        (inverted & (inverted.wrapping_add(1))) == 0
    }

    fn netmask_to_cidr(netmask: &str) -> Result<u8, NetworkError> {
        let parts: Vec<&str> = netmask.split('.').collect();
        if parts.len() != 4 {
            return Err(NetworkError::CommandFailed(
                "Invalid netmask format".to_string(),
            ));
        }

        let octets: Result<Vec<u8>, _> = parts.iter().map(|p| p.parse::<u8>()).collect();
        let octets = octets
            .map_err(|_| NetworkError::CommandFailed("Invalid netmask octets".to_string()))?;

        let mut cidr = 0u8;
        for octet in octets {
            for i in (0..8).rev() {
                if (octet >> i) & 1 == 1 {
                    cidr += 1;
                } else {
                    return Ok(cidr);
                }
            }
        }

        Ok(cidr)
    }

    fn cidr_to_netmask(cidr: u8) -> String {
        if cidr > 32 {
            return "255.255.255.0".to_string();
        }

        let mask = if cidr == 0 {
            0u32
        } else {
            0xFFFFFFFF << (32 - cidr)
        };

        format!(
            "{}.{}.{}.{}",
            (mask >> 24) & 0xFF,
            (mask >> 16) & 0xFF,
            (mask >> 8) & 0xFF,
            mask & 0xFF
        )
    }

    async fn save_ethernet_config(config: &EthernetConfig) -> Result<(), NetworkError> {
        fs::create_dir_all("/etc/kvm").await?;
        let yaml = serde_yaml::to_string(config)
            .map_err(|e| NetworkError::CommandFailed(e.to_string()))?;
        fs::write(ETH_CONFIG_FILE, yaml).await?;
        Ok(())
    }

    pub async fn read_saved_config() -> EthernetConfig {
        match fs::read_to_string(ETH_CONFIG_FILE).await {
            Ok(yaml) => serde_yaml::from_str(&yaml).unwrap_or_else(|_| EthernetConfig {
                dhcp: true,
                ip: None,
                netmask: None,
                gateway: None,
                dns1: None,
                dns2: None,
            }),
            Err(_) => {
                let dhcp = !Path::new(ETH_NODHCP_FILE).exists();
                EthernetConfig {
                    dhcp,
                    ip: None,
                    netmask: None,
                    gateway: None,
                    dns1: None,
                    dns2: None,
                }
            }
        }
    }

    pub async fn get_current_config() -> EthernetConfig {
        let dhcp = !Path::new(ETH_NODHCP_FILE).exists();
        let (ip, netmask, gateway) = Self::get_eth0_config().await;
        let (dns1, dns2) = Self::get_dns_servers().await;

        EthernetConfig {
            dhcp,
            ip,
            netmask,
            gateway,
            dns1,
            dns2,
        }
    }

    async fn get_eth0_config() -> (Option<String>, Option<String>, Option<String>) {
        let output = Command::new("ip")
            .args(["addr", "show", "eth0"])
            .output()
            .await;

        let (mut ip, mut netmask) = (None, None);

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("inet ") && !line.contains("inet6") {
                    if let Some(addr_part) = line.split_whitespace().nth(1) {
                        if let Some((addr, cidr)) = addr_part.split_once('/') {
                            ip = Some(addr.to_string());
                            if let Ok(cidr_num) = cidr.parse::<u8>() {
                                netmask = Some(Self::cidr_to_netmask(cidr_num));
                            }
                        }
                    }
                }
            }
        }

        let gateway = Self::get_default_gateway().await;
        (ip, netmask, gateway)
    }

    async fn get_default_gateway() -> Option<String> {
        let output = Command::new("ip")
            .args(["route", "show", "default"])
            .output()
            .await;

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let parts: Vec<&str> = stdout.split_whitespace().collect();
            for i in 0..parts.len() {
                if parts[i] == "via" && i + 1 < parts.len() {
                    return Some(parts[i + 1].to_string());
                }
            }
        }

        None
    }

    async fn get_dns_servers() -> (Option<String>, Option<String>) {
        let content = fs::read_to_string(RESOLV_CONF_FILE).await;
        let mut dns1 = None;
        let mut dns2 = None;

        if let Ok(content) = content {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with("nameserver ") {
                    let server = line.trim_start_matches("nameserver ").trim();
                    if dns1.is_none() {
                        dns1 = Some(server.to_string());
                    } else if dns2.is_none() {
                        dns2 = Some(server.to_string());
                        break;
                    }
                }
            }
        }

        (dns1, dns2)
    }

    async fn write_dns_config(dns1: Option<&str>, dns2: Option<&str>) -> Result<(), NetworkError> {
        let mut content = String::new();

        if let Some(dns) = dns1 {
            if !dns.is_empty() && Self::is_valid_ip(dns) {
                content.push_str(&format!("nameserver {}\n", dns));
            }
        }

        if let Some(dns) = dns2 {
            if !dns.is_empty() && Self::is_valid_ip(dns) {
                content.push_str(&format!("nameserver {}\n", dns));
            }
        }

        if !content.is_empty() {
            fs::write(RESOLV_CONF_FILE, content).await?;
        }

        Ok(())
    }

    async fn restart_ethernet() {
        info!("Restarting ethernet service...");

        let _ = Command::new("sh")
            .arg("-c")
            .arg(format!("{} stop", ETH_INIT_SCRIPT))
            .status()
            .await;

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        match Command::new("sh")
            .arg("-c")
            .arg(format!("{} start", ETH_INIT_SCRIPT))
            .output()
            .await
        {
            Ok(output) if output.status.success() => {
                info!("Ethernet restarted successfully");
            }
            Ok(output) => {
                error!(
                    "Ethernet restart failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Err(e) => {
                error!("Failed to restart ethernet: {}", e);
            }
        }
    }

    pub async fn set_ethernet_config(req: SetEthernetConfigReq) -> Result<(), NetworkError> {
        if req.dhcp {
            let _ = fs::remove_file(ETH_NODHCP_FILE).await;
            let config = EthernetConfig {
                dhcp: true,
                ip: None,
                netmask: None,
                gateway: None,
                dns1: None,
                dns2: None,
            };
            Self::save_ethernet_config(&config).await?;
        } else {
            let ip = req.ip.ok_or_else(|| {
                NetworkError::CommandFailed("IP address required for static mode".to_string())
            })?;
            let netmask = req.netmask.ok_or_else(|| {
                NetworkError::CommandFailed("Netmask required for static mode".to_string())
            })?;
            let gateway = req.gateway.ok_or_else(|| {
                NetworkError::CommandFailed("Gateway required for static mode".to_string())
            })?;

            if !Self::is_valid_ip(&ip) {
                return Err(NetworkError::CommandFailed(
                    "Invalid IP address".to_string(),
                ));
            }
            if !Self::is_valid_netmask(&netmask) {
                return Err(NetworkError::CommandFailed("Invalid netmask".to_string()));
            }
            if !Self::is_valid_ip(&gateway) {
                return Err(NetworkError::CommandFailed("Invalid gateway".to_string()));
            }

            if let Some(ref dns) = req.dns1 {
                if !dns.is_empty() && !Self::is_valid_ip(dns) {
                    return Err(NetworkError::CommandFailed("Invalid DNS1".to_string()));
                }
            }
            if let Some(ref dns) = req.dns2 {
                if !dns.is_empty() && !Self::is_valid_ip(dns) {
                    return Err(NetworkError::CommandFailed("Invalid DNS2".to_string()));
                }
            }

            let cidr = Self::netmask_to_cidr(&netmask)?;
            let content = format!("{}/{} {}\n", ip, cidr, gateway);
            fs::write(ETH_NODHCP_FILE, content).await?;

            Self::write_dns_config(req.dns1.as_deref(), req.dns2.as_deref()).await?;

            let config = EthernetConfig {
                dhcp: false,
                ip: Some(ip),
                netmask: Some(netmask),
                gateway: Some(gateway),
                dns1: req.dns1,
                dns2: req.dns2,
            };
            Self::save_ethernet_config(&config).await?;
        }

        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            Self::restart_ethernet().await;
        });

        Ok(())
    }
}
