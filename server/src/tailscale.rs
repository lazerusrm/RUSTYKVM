#![allow(dead_code)]

use crate::api::ApiResponse;
use axum::{extract::Json, response::IntoResponse};
use serde::Serialize;
use tokio::process::Command;
use tracing::{error, info};

const SCRIPT_PATH: &str = "/etc/init.d/S98tailscaled";
const SCRIPT_BACKUP_PATH: &str = "/kvmapp/system/init.d/S98tailscaled";
const TAILSCALE_PATH: &str = "/usr/bin/tailscale";
const TAILSCALED_PATH: &str = "/usr/sbin/tailscaled";
const ORIGINAL_URL: &str = "https://pkgs.tailscale.com/stable/tailscale_latest_riscv64.tgz";
const WORKSPACE: &str = "/root/.tailscale";

use serde::Deserialize;

/// Raw tailscale status JSON structure (for deserialization)
#[derive(Deserialize)]
struct TsStatusRaw {
    #[serde(rename = "BackendState")]
    backend_state: String,
    #[serde(rename = "Self")]
    self_node: Option<TsSelfNodeRaw>,
    #[serde(rename = "CurrentTailnet")]
    current_tailnet: Option<TsTailnetRaw>,
}

#[derive(Deserialize)]
struct TsSelfNodeRaw {
    #[serde(rename = "HostName")]
    host_name: Option<String>,
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Option<Vec<String>>,
    #[serde(rename = "Online")]
    online: Option<bool>,
}

#[derive(Deserialize)]
struct TsTailnetRaw {
    #[serde(rename = "Name")]
    name: Option<String>,
}

/// Simplified status response
#[derive(Serialize)]
pub struct TsStatusResponse {
    pub installed: bool,
    pub running: bool,
    pub connected: bool,
    pub backend_state: String,
    pub hostname: String,
    pub tailnet_name: String,
    pub ips: Vec<String>,
}

#[derive(Serialize)]
pub struct LoginRsp {
    pub url: String,
}

/// Go-frontend status response shape (`/api/extensions/tailscale/status`)
#[derive(Debug, Serialize)]
pub struct GetTailscaleStatusRsp {
    pub state: String, // notInstall | notRunning | notLogin | stopped | running
    pub name: String,
    pub ip: String,
    pub account: String,
}

fn ui_state_from_backend_state(state: &str) -> &'static str {
    match state {
        "NoState" | "Starting" => "notRunning",
        "NeedsLogin" => "notLogin",
        "Running" => "running",
        "Stopped" => "stopped",
        _ => "notRunning",
    }
}

pub async fn tailscale_start_handler() -> impl IntoResponse {
    let cmd = format!(
        "cp -f {} {} && {} start",
        SCRIPT_BACKUP_PATH, SCRIPT_PATH, SCRIPT_PATH
    );
    match Command::new("sh").arg("-c").arg(cmd).status().await {
        Ok(s) if s.success() => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        _ => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, "start failed")).into_response(),
    }
}

pub async fn tailscale_restart_handler() -> impl IntoResponse {
    let cmd = format!(
        "cp -f {} {} && {} restart",
        SCRIPT_BACKUP_PATH, SCRIPT_PATH, SCRIPT_PATH
    );
    match Command::new("sh").arg("-c").arg(cmd).status().await {
        Ok(s) if s.success() => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        _ => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, "restart failed")).into_response(),
    }
}

pub async fn tailscale_stop_handler() -> impl IntoResponse {
    let cmd = format!("{} stop && rm -f {}", SCRIPT_PATH, SCRIPT_PATH);
    match Command::new("sh").arg("-c").arg(cmd).status().await {
        Ok(s) if s.success() => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        _ => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, "stop failed")).into_response(),
    }
}

pub async fn tailscale_status_handler() -> impl IntoResponse {
    // Check if tailscale is installed
    let installed = tokio::fs::metadata(TAILSCALE_PATH).await.is_ok();
    if !installed {
        return Json(ApiResponse::ok(GetTailscaleStatusRsp {
            state: "notInstall".to_string(),
            name: String::new(),
            ip: String::new(),
            account: String::new(),
        }))
        .into_response();
    }

    let output = Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await;

    let status: Option<TsStatusRaw> = match output {
        Ok(out) if out.status.success() => serde_json::from_slice::<TsStatusRaw>(&out.stdout).ok(),
        _ => None,
    };

    let Some(status) = status else {
        return Json(ApiResponse::ok(GetTailscaleStatusRsp {
            state: "notRunning".to_string(),
            name: String::new(),
            ip: String::new(),
            account: String::new(),
        }))
        .into_response();
    };

    let self_node = status.self_node.as_ref();
    let tailnet = status.current_tailnet.as_ref();

    let mut ipv4 = String::new();
    if let Some(ips) = self_node.and_then(|n| n.tailscale_ips.as_ref()) {
        for ip in ips {
            if let Ok(addr) = ip.parse::<std::net::IpAddr>() {
                if let std::net::IpAddr::V4(v4) = addr {
                    ipv4 = v4.to_string();
                    break;
                }
            }
        }
    }

    Json(ApiResponse::ok(GetTailscaleStatusRsp {
        state: ui_state_from_backend_state(&status.backend_state).to_string(),
        name: self_node
            .and_then(|n| n.host_name.clone())
            .unwrap_or_default(),
        ip: ipv4,
        account: tailnet.and_then(|t| t.name.clone()).unwrap_or_default(),
    }))
    .into_response()
}

pub async fn tailscale_login_handler() -> impl IntoResponse {
    // If tailscale is already running, frontend expects an empty URL.
    if let Ok(out) = Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await
    {
        if out.status.success() {
            if let Ok(status) = serde_json::from_slice::<TsStatusRaw>(&out.stdout) {
                if status.backend_state == "Running" {
                    return Json(ApiResponse::ok(LoginRsp { url: String::new() })).into_response();
                }
            }
        }
    }

    // We use tokio::process::Command with a timeout
    let cmd_future = Command::new("sh")
        .arg("-c")
        .arg("tailscale login --accept-dns=false --timeout=30s")
        .output();

    match tokio::time::timeout(std::time::Duration::from_secs(35), cmd_future).await {
        Ok(Ok(out)) => {
            // Tailscale login URL can be in stderr or stdout
            let err_s = String::from_utf8_lossy(&out.stderr);
            let out_s = String::from_utf8_lossy(&out.stdout);

            let combined = format!("{}{}", err_s, out_s);
            for line in combined.lines() {
                if line.contains("https://") {
                    if let Some(url_idx) = line.find("https://") {
                        let url = line[url_idx..]
                            .split_whitespace()
                            .next()
                            .unwrap_or_default();
                        if url.starts_with("https://login.tailscale.com") {
                            return Json(ApiResponse::ok(LoginRsp {
                                url: url.to_string(),
                            }))
                            .into_response();
                        }
                    }
                }
            }
            error!("Tailscale login URL not found in output: {}", combined);
            Json(ApiResponse::<serde_json::Value>::err(-2, "login failed")).into_response()
        }
        Ok(Err(e)) => {
            Json(ApiResponse::<serde_json::Value>::err(-2, &e.to_string())).into_response()
        }
        Err(_) => Json(ApiResponse::<serde_json::Value>::err(
            -2,
            "tailscale command timed out",
        ))
        .into_response(),
    }
}

pub async fn tailscale_up_handler() -> impl IntoResponse {
    match Command::new("sh")
        .arg("-c")
        .arg("tailscale up --accept-dns=false")
        .status()
        .await
    {
        Ok(s) if s.success() => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        _ => Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "tailscale up failed",
        ))
        .into_response(),
    }
}

pub async fn tailscale_down_handler() -> impl IntoResponse {
    match Command::new("sh")
        .arg("-c")
        .arg("tailscale down")
        .status()
        .await
    {
        Ok(s) if s.success() => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        _ => Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "tailscale down failed",
        ))
        .into_response(),
    }
}

pub async fn tailscale_logout_handler() -> impl IntoResponse {
    match Command::new("sh")
        .arg("-c")
        .arg("tailscale logout")
        .status()
        .await
    {
        Ok(s) if s.success() => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        _ => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, "logout failed")).into_response(),
    }
}

pub async fn tailscale_install_handler() -> impl IntoResponse {
    tokio::spawn(async move {
        if let Err(e) = perform_tailscale_install().await {
            error!("Tailscale installation failed: {}", e);
        }
    });
    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

pub async fn tailscale_uninstall_handler() -> impl IntoResponse {
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("{} stop", SCRIPT_PATH))
        .status()
        .await;
    let _ = tokio::fs::remove_file(TAILSCALE_PATH).await;
    let _ = tokio::fs::remove_file(TAILSCALED_PATH).await;
    let _ = tokio::fs::remove_file(SCRIPT_PATH).await;
    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

async fn perform_tailscale_install() -> anyhow::Result<()> {
    let _ = tokio::fs::create_dir_all(WORKSPACE).await;
    let tar_file = format!("{}/tailscale.tgz", WORKSPACE);

    // 1. Download
    let resp = reqwest::get(ORIGINAL_URL).await?;
    let bytes = resp.bytes().await?;
    tokio::fs::write(&tar_file, &bytes).await?;

    // 2. Extract (using spawn_blocking for long synchronous operation)
    let tar_file_clone = tar_file.clone();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&tar_file_clone)?;
        let tar = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(tar);
        archive.unpack(WORKSPACE)
    })
    .await??;

    // 3. Move binaries (finding them in the extracted dir)
    let mut entries = tokio::fs::read_dir(WORKSPACE).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            let dir_path = entry.path();
            let ts = dir_path.join("tailscale");
            let tsd = dir_path.join("tailscaled");
            if ts.exists() && tsd.exists() {
                tokio::fs::copy(&ts, TAILSCALE_PATH).await?;
                tokio::fs::copy(&tsd, TAILSCALED_PATH).await?;
                let _ = Command::new("chmod")
                    .arg("755")
                    .arg(TAILSCALE_PATH)
                    .status()
                    .await;
                let _ = Command::new("chmod")
                    .arg("755")
                    .arg(TAILSCALED_PATH)
                    .status()
                    .await;
                break;
            }
        }
    }

    let _ = tokio::fs::remove_dir_all(WORKSPACE).await;
    info!("Tailscale installed successfully");
    Ok(())
}

// ============================================================================
// Tailscale Auto-Update Feature
// ============================================================================

/// Response from tailscale debug prefs command
#[derive(Deserialize)]
struct TsPrefs {
    #[serde(rename = "AutoUpdate")]
    auto_update: Option<TsAutoUpdate>,
}

#[derive(Deserialize)]
struct TsAutoUpdate {
    #[serde(rename = "Apply")]
    apply: Option<bool>,
}

/// Request/Response structures for auto-update API
#[derive(Debug, Serialize)]
pub struct GetAutoUpdateRsp {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetAutoUpdateReq {
    #[serde(rename = "enable", alias = "enabled")]
    pub enabled: bool,
}

/// Helper function to check if Tailscale is installed
fn is_tailscale_installed() -> bool {
    std::path::Path::new(TAILSCALE_PATH).exists()
}

/// Get auto-update status from tailscale debug prefs
async fn get_tailscale_auto_update() -> Result<bool, String> {
    let output = Command::new("tailscale")
        .args(["debug", "prefs"])
        .output()
        .await
        .map_err(|e| format!("Failed to execute tailscale: {}", e))?;

    if !output.status.success() {
        return Err("tailscale command failed".to_string());
    }

    // Parse JSON output
    let json_str = String::from_utf8_lossy(&output.stdout);
    let prefs: TsPrefs =
        serde_json::from_str(&json_str).map_err(|e| format!("Failed to parse JSON: {}", e))?;

    // Extract AutoUpdate.Apply field
    Ok(prefs.auto_update.and_then(|au| au.apply).unwrap_or(false))
}

/// Set auto-update status
async fn set_tailscale_auto_update(enabled: bool) -> Result<(), String> {
    let arg = if enabled { "true" } else { "false" };
    let cmd = format!("tailscale set --auto-update={}", arg);

    let status = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .status()
        .await
        .map_err(|e| format!("Failed to execute command: {}", e))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Failed to set auto-update: command '{}'returned error",
            cmd
        ))
    }
}

// ============================================================================
// Public Detection Functions (for use in capabilities endpoint)
// ============================================================================

/// Check if Tailscale binary is installed
pub async fn check_tailscale_installed() -> bool {
    tokio::fs::metadata(TAILSCALE_PATH).await.is_ok()
}

/// Check if Tailscale is connected to the network
pub async fn check_tailscale_connected() -> bool {
    let output = Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());

    if let Some(json) = output {
        // Check for Online: true in various JSON formats
        json.contains("\"Online\":true") || json.contains("\"Online\": true")
    } else {
        false
    }
}

/// Check if Tailscale Funnel is active
pub async fn check_funnel_active() -> bool {
    let output = Command::new("tailscale")
        .args(["serve", "status"])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());

    if let Some(status) = output {
        status.contains("https://") || status.contains("funnel")
    } else {
        false
    }
}

/// Get the Funnel URL if active
pub async fn get_funnel_url() -> Option<String> {
    let output = Command::new("tailscale")
        .args(["serve", "status"])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok());

    if let Some(status) = output {
        if let Some(start) = status.find("https://") {
            let url_part = &status[start..];
            let end = url_part.find(['\n', ' ', '\t']).unwrap_or(url_part.len());
            let url = url_part[..end].trim();
            if url.contains(".ts.net") {
                return Some(url.to_string());
            }
        }
    }
    None
}

/// Combined capabilities check
#[derive(Serialize)]
pub struct TailscaleCapabilities {
    pub installed: bool,
    pub connected: bool,
    pub funnel_active: bool,
    pub funnel_url: Option<String>,
}

pub async fn get_capabilities() -> TailscaleCapabilities {
    let installed = check_tailscale_installed().await;
    if !installed {
        return TailscaleCapabilities {
            installed: false,
            connected: false,
            funnel_active: false,
            funnel_url: None,
        };
    }

    let connected = check_tailscale_connected().await;
    let funnel_active = check_funnel_active().await;
    let funnel_url = if funnel_active {
        get_funnel_url().await
    } else {
        None
    };

    TailscaleCapabilities {
        installed,
        connected,
        funnel_active,
        funnel_url,
    }
}

// ============================================================================
// Background Auto-Updater
// ============================================================================

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static AUTO_UPDATE_ENABLED: AtomicBool = AtomicBool::new(false);
const AUTO_UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60); // 24 hours
const AUTO_UPDATE_SETTINGS_FILE: &str = "/etc/kvm/tailscale_settings.json";

#[derive(Serialize, Deserialize, Default)]
struct TailscaleSettings {
    auto_update_enabled: bool,
}

async fn load_settings() -> TailscaleSettings {
    match tokio::fs::read_to_string(AUTO_UPDATE_SETTINGS_FILE).await {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => TailscaleSettings::default(),
    }
}

async fn save_settings(settings: &TailscaleSettings) -> Result<(), std::io::Error> {
    if let Some(parent) = std::path::Path::new(AUTO_UPDATE_SETTINGS_FILE).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let json = serde_json::to_string_pretty(settings)?;
    tokio::fs::write(AUTO_UPDATE_SETTINGS_FILE, json).await
}

/// Initialize auto-update from saved settings
pub async fn init_auto_update() {
    let settings = load_settings().await;
    AUTO_UPDATE_ENABLED.store(settings.auto_update_enabled, Ordering::SeqCst);
    info!(
        "Tailscale auto-update initialized: enabled={}",
        settings.auto_update_enabled
    );
}

/// Start background auto-update checker task
pub fn spawn_auto_update_task() {
    tokio::spawn(async move {
        // Initial delay before first check
        tokio::time::sleep(Duration::from_secs(60)).await;

        loop {
            if AUTO_UPDATE_ENABLED.load(Ordering::SeqCst) {
                if check_tailscale_installed().await {
                    info!("Running Tailscale auto-update check...");
                    match perform_tailscale_update().await {
                        Ok(updated) => {
                            if updated {
                                info!("Tailscale was updated successfully");
                            } else {
                                info!("Tailscale is already up to date");
                            }
                        }
                        Err(e) => {
                            error!("Tailscale auto-update failed: {}", e);
                        }
                    }
                }
            }
            tokio::time::sleep(AUTO_UPDATE_CHECK_INTERVAL).await;
        }
    });
}

/// Perform tailscale update - returns true if updated, false if already up to date
async fn perform_tailscale_update() -> anyhow::Result<bool> {
    // Check current version
    let current_version = get_current_version().await;

    // Try to update using tailscale update command (if available)
    let update_result = Command::new("tailscale")
        .args(["update", "--yes"])
        .output()
        .await;

    match update_result {
        Ok(output) => {
            let _stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if output.status.success() {
                // Check if version changed
                let new_version = get_current_version().await;
                if new_version != current_version {
                    info!(
                        "Tailscale updated from {:?} to {:?}",
                        current_version, new_version
                    );
                    return Ok(true);
                }
                return Ok(false);
            }

            // If update command not available, try manual update
            if stderr.contains("unknown command") || stderr.contains("not recognized") {
                info!("Tailscale update command not available, trying manual update");
                return perform_manual_update().await;
            }

            Err(anyhow::anyhow!("Update failed: {}", stderr))
        }
        Err(e) => Err(anyhow::anyhow!("Failed to run tailscale update: {}", e)),
    }
}

async fn get_current_version() -> Option<String> {
    Command::new("tailscale")
        .args(["version"])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.lines().next().unwrap_or("").to_string())
}

async fn perform_manual_update() -> anyhow::Result<bool> {
    // Download latest release
    let _ = tokio::fs::create_dir_all(WORKSPACE).await;
    let tar_file = format!("{}/tailscale.tgz", WORKSPACE);

    let resp = reqwest::get(ORIGINAL_URL).await?;
    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Failed to download update"));
    }

    let bytes = resp.bytes().await?;
    tokio::fs::write(&tar_file, &bytes).await?;

    // Extract
    let tar_file_clone = tar_file.clone();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&tar_file_clone)?;
        let tar = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(tar);
        archive.unpack(WORKSPACE)
    })
    .await??;

    // Find and copy binaries
    let mut entries = tokio::fs::read_dir(WORKSPACE).await?;
    let mut updated = false;

    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            let dir_path = entry.path();
            let ts = dir_path.join("tailscale");
            let tsd = dir_path.join("tailscaled");

            if ts.exists() && tsd.exists() {
                // Stop tailscaled before replacing
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg(format!("{} stop 2>/dev/null || true", SCRIPT_PATH))
                    .status()
                    .await;

                tokio::fs::copy(&ts, TAILSCALE_PATH).await?;
                tokio::fs::copy(&tsd, TAILSCALED_PATH).await?;

                let _ = Command::new("chmod")
                    .arg("755")
                    .arg(TAILSCALE_PATH)
                    .status()
                    .await;
                let _ = Command::new("chmod")
                    .arg("755")
                    .arg(TAILSCALED_PATH)
                    .status()
                    .await;

                // Restart tailscaled
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg(format!("{} start 2>/dev/null || true", SCRIPT_PATH))
                    .status()
                    .await;

                updated = true;
                break;
            }
        }
    }

    let _ = tokio::fs::remove_dir_all(WORKSPACE).await;

    if updated {
        info!("Manual Tailscale update completed");
        Ok(true)
    } else {
        Err(anyhow::anyhow!(
            "Could not find tailscale binaries in download"
        ))
    }
}

/// HTTP handler to get auto-update status
#[cfg(target_os = "linux")]
pub async fn get_auto_update_handler() -> impl IntoResponse {
    // Check if Tailscale is installed
    if !is_tailscale_installed() {
        return Json(ApiResponse::<GetAutoUpdateRsp>::err(
            -1,
            "tailscale not installed",
        ))
        .into_response();
    }

    let enabled = AUTO_UPDATE_ENABLED.load(Ordering::SeqCst);
    info!("Tailscale auto-update status: {}", enabled);
    Json(ApiResponse::ok(GetAutoUpdateRsp { enabled })).into_response()
}

/// HTTP handler to set auto-update status
#[cfg(target_os = "linux")]
pub async fn set_auto_update_handler(Json(req): Json<SetAutoUpdateReq>) -> impl IntoResponse {
    // Check if Tailscale is installed
    if !is_tailscale_installed() {
        return Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "tailscale not installed",
        ))
        .into_response();
    }

    // Update in-memory state
    AUTO_UPDATE_ENABLED.store(req.enabled, Ordering::SeqCst);

    // Persist to disk
    let settings = TailscaleSettings {
        auto_update_enabled: req.enabled,
    };
    if let Err(e) = save_settings(&settings).await {
        error!("Failed to save auto-update settings: {}", e);
    }

    // Also set the tailscale built-in auto-update if available
    let _ = set_tailscale_auto_update(req.enabled).await;

    info!("Tailscale auto-update set to: {}", req.enabled);
    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

/// HTTP handler to manually trigger an update check
#[cfg(target_os = "linux")]
pub async fn trigger_update_handler() -> impl IntoResponse {
    if !is_tailscale_installed() {
        return Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "tailscale not installed",
        ))
        .into_response();
    }

    // Spawn update in background
    tokio::spawn(async {
        match perform_tailscale_update().await {
            Ok(updated) => {
                if updated {
                    info!("Manual Tailscale update completed successfully");
                } else {
                    info!("Tailscale is already up to date");
                }
            }
            Err(e) => {
                error!("Manual Tailscale update failed: {}", e);
            }
        }
    });

    Json(ApiResponse::ok(
        serde_json::json!({"status": "update_started"}),
    ))
    .into_response()
}

/// HTTP handler to get Tailscale version info
#[cfg(target_os = "linux")]
pub async fn get_version_handler() -> impl IntoResponse {
    if !is_tailscale_installed() {
        return Json(ApiResponse::ok(serde_json::json!({
            "installed": false,
            "version": null
        })))
        .into_response();
    }

    let version = get_current_version().await;
    Json(ApiResponse::ok(serde_json::json!({
        "installed": true,
        "version": version
    })))
    .into_response()
}
