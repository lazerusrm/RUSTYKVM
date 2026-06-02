//! EDID read / write support for strict parity with the official Go NanoKVM firmware.
//!
//! Go primarily exposes EDID management through the `nanokvm_update_edid` tool
//! (direct I2C to the LT6911 receiver). This module provides equivalent API access.

#[cfg(target_os = "linux")]
#[path = "edid_i2c.rs"]
mod edid_i2c;

use crate::api::{error_codes, ApiResponse};
use axum::{
    extract::Json,
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose, Engine};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Path to the EDID management tool (matches where it typically ends up on the device).
const EDID_TOOL: &str = "/usr/local/bin/nanokvm_update_edid";

/// Standard EDID size (256 bytes for extended).
const EDID_SIZE: usize = 256;

/// Persistence path for custom EDID (re-applied on boot for parity with Go behavior).
const CUSTOM_EDID_FILE: &str = "/etc/kvm/edid.bin";

/// Built-in 1080p template shipped with the official `nanokvm_update_edid` tool.
const DEFAULT_EDID_BIN: &[u8] = include_bytes!("../../assets/edid/E21_NanoKVM.bin");

/// Response for current EDID.
#[derive(Debug, Serialize)]
pub struct GetEdidRsp {
    /// Base64 encoded EDID blob (always 256 bytes when present).
    pub data: Option<String>,
    /// Human readable status / chip info.
    pub status: String,
    /// Whether a custom EDID is currently active (vs factory default).
    pub custom: bool,
}

/// Request to write a new EDID.
#[derive(Debug, Deserialize)]
pub struct SetEdidReq {
    /// Base64 encoded 256-byte EDID.
    pub data: String,
}

/// Response after applying default 1080p template.
#[derive(Debug, Serialize)]
pub struct ApplyDefaultEdidRsp {
    pub status: String,
}

/// Validates basic EDID structure (header + checksums).
/// Mirrors the checks performed by the official `nanokvm_update_edid` tool.
pub fn validate_edid(data: &[u8]) -> Result<(), String> {
    if data.len() != EDID_SIZE {
        return Err(format!("EDID must be exactly {} bytes", EDID_SIZE));
    }

    let header = [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00];
    if data[0..8] != header {
        return Err("Invalid EDID header".to_string());
    }

    let mut sum1: u8 = 0;
    for b in &data[0..127] {
        sum1 = sum1.wrapping_add(*b);
    }
    let checksum1 = 0x100u16.wrapping_sub(sum1 as u16) as u8;
    if checksum1 != data[127] {
        return Err("Checksum for first 128 bytes is incorrect".to_string());
    }

    let mut sum2: u8 = 0;
    for b in &data[128..255] {
        sum2 = sum2.wrapping_add(*b);
    }
    let checksum2 = 0x100u16.wrapping_sub(sum2 as u16) as u8;
    if checksum2 != data[255] {
        return Err("Checksum for second 128 bytes is incorrect".to_string());
    }

    Ok(())
}

async fn save_custom_edid(data: &[u8]) -> std::io::Result<()> {
    let tmp = format!("{}.tmp", CUSTOM_EDID_FILE);
    tokio::fs::write(&tmp, data).await?;
    tokio::fs::rename(&tmp, CUSTOM_EDID_FILE).await
}

async fn custom_edid_persisted() -> bool {
    tokio::fs::metadata(CUSTOM_EDID_FILE).await.is_ok()
}

/// Load persisted custom EDID if present.
pub async fn load_and_apply_custom_edid_on_boot() {
    if let Ok(data) = tokio::fs::read(CUSTOM_EDID_FILE).await {
        if data.len() == EDID_SIZE && validate_edid(&data).is_ok() {
            if let Err(e) = write_edid_via_tool(&data).await {
                warn!("Failed to re-apply persisted custom EDID on boot: {}", e);
            } else {
                info!("Re-applied custom EDID from {}", CUSTOM_EDID_FILE);
            }
        }
    }
}

async fn read_edid_via_i2c() -> Result<Vec<u8>, String> {
    #[cfg(target_os = "linux")]
    {
        edid_i2c::read_edid_from_hardware().await
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = ();
        Err("EDID hardware read is only supported on Linux".to_string())
    }
}

/// Write EDID using the official tool (exact Go parity path).
async fn write_edid_via_tool(data: &[u8]) -> Result<(), String> {
    if !std::path::Path::new(EDID_TOOL).exists() {
        return Err(
            "EDID tool not present on this device (expected at /usr/local/bin/nanokvm_update_edid)"
                .to_string(),
        );
    }

    let tmp = "/tmp/kvm_custom.edid";
    tokio::fs::write(tmp, data)
        .await
        .map_err(|e| format!("Failed to write temporary EDID file: {}", e))?;

    let status = tokio::process::Command::new(EDID_TOOL)
        .arg(tmp)
        .status()
        .await
        .map_err(|e| format!("Failed to execute EDID tool: {}", e))?;

    let _ = tokio::fs::remove_file(tmp).await;

    if !status.success() {
        return Err(
            "EDID tool failed to program the new EDID (see device logs for details)".to_string(),
        );
    }

    Ok(())
}

fn ok_edid_response(data: Vec<u8>, custom: bool) -> Response {
    let encoded = general_purpose::STANDARD.encode(&data);
    Json(ApiResponse::ok(GetEdidRsp {
        data: Some(encoded),
        status: "ok".to_string(),
        custom,
    }))
    .into_response()
}

pub async fn get_edid_handler() -> impl IntoResponse {
    let custom = custom_edid_persisted().await;

    match read_edid_via_i2c().await {
        Ok(data) => {
            if let Err(e) = validate_edid(&data) {
                return Json(ApiResponse::<GetEdidRsp>::err(
                    error_codes::HARDWARE,
                    &format!("Read EDID failed validation: {}", e),
                ))
                .into_response();
            }
            ok_edid_response(data, custom)
        }
        Err(hw_err) => {
            // Fall back to last persisted custom EDID when direct I2C read is unavailable.
            if let Ok(data) = tokio::fs::read(CUSTOM_EDID_FILE).await {
                if data.len() == EDID_SIZE && validate_edid(&data).is_ok() {
                    warn!(
                        "EDID hardware read failed ({}); returning persisted custom EDID",
                        hw_err
                    );
                    return ok_edid_response(data, true);
                }
            }
            Json(ApiResponse::<GetEdidRsp>::err(
                error_codes::HARDWARE,
                &hw_err,
            ))
            .into_response()
        }
    }
}

pub async fn set_edid_handler(Json(req): Json<SetEdidReq>) -> impl IntoResponse {
    let data = match general_purpose::STANDARD.decode(&req.data) {
        Ok(d) => d,
        Err(_) => {
            return Json(ApiResponse::<serde_json::Value>::err(
                error_codes::VALIDATION,
                "invalid base64 EDID data",
            ))
            .into_response();
        }
    };

    if let Err(e) = validate_edid(&data) {
        return Json(ApiResponse::<serde_json::Value>::err(
            error_codes::VALIDATION,
            &e,
        ))
        .into_response();
    }

    match write_edid_via_tool(&data).await {
        Ok(()) => {
            if let Err(e) = save_custom_edid(&data).await {
                warn!("EDID written successfully but failed to persist: {}", e);
            }
            Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::err(
            error_codes::HARDWARE,
            &format!("Failed to write EDID: {}", e),
        ))
        .into_response(),
    }
}

pub async fn apply_default_edid_handler() -> impl IntoResponse {
    let data = DEFAULT_EDID_BIN;
    if data.len() != EDID_SIZE {
        return Json(ApiResponse::<ApplyDefaultEdidRsp>::err(
            error_codes::GENERIC,
            "built-in default EDID template has invalid size",
        ))
        .into_response();
    }

    if let Err(e) = validate_edid(data) {
        return Json(ApiResponse::<ApplyDefaultEdidRsp>::err(
            error_codes::GENERIC,
            &format!("built-in default EDID template is invalid: {}", e),
        ))
        .into_response();
    }

    match write_edid_via_tool(data).await {
        Ok(()) => {
            let _ = tokio::fs::remove_file(CUSTOM_EDID_FILE).await;
            Json(ApiResponse::ok(ApplyDefaultEdidRsp {
                status: "default 1080p EDID applied".to_string(),
            }))
            .into_response()
        }
        Err(e) => Json(ApiResponse::<ApplyDefaultEdidRsp>::err(
            error_codes::HARDWARE,
            &format!("Failed to apply default EDID: {}", e),
        ))
        .into_response(),
    }
}
