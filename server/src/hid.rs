use axum {
    extract::{State, Json},
    response::IntoResponse,
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info, debug};
use crate::AppState;
use hid::{Shortcut, ShortcutKey};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct PasteReq {
    pub content: String,
    pub langue: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GetShortcutsRsp {
    pub shortcuts: Vec<Shortcut>,
}

#[derive(Debug, Deserialize)]
pub struct AddShortcutReq {
    pub keys: Vec<ShortcutKey>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteShortcutReq {
    pub id: String,
}

#[derive(Debug, Serialize)]
pub struct GetHidModeRsp {
    pub mode: String,
}

#[derive(Debug, Deserialize)]
pub struct SetHidModeReq {
    pub mode: String,
}

const SHORTCUT_FILE: &str = "/etc/kvm/shortcuts.json";
const HID_MODE_FLAG: &str = "/sys/kernel/config/usb_gadget/g0/bcdDevice";

pub async fn paste_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasteReq>,
) -> impl IntoResponse {
    if req.content.len() > 1024 {
        return (StatusCode::BAD_REQUEST, "Content too long").into_response();
    }

    let char_map = get_char_map(req.langue.as_deref().unwrap_or(""));
    let mut hid = state.hid.lock().await;

    let key_up = [0u8; 8];

    for c in req.content.chars() {
        if let Some((mods, code)) = char_map.get(&c) {
            let key_down = [*mods, 0, *code, 0, 0, 0, 0, 0];
            let _ = hid.send_keyboard(&key_down).await;
            let _ = hid.send_keyboard(&key_up).await;
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    }

    StatusCode::OK.into_response()
}

pub async fn get_shortcuts_handler() -> impl IntoResponse {
    let shortcuts = load_shortcuts().await.unwrap_or_default();
    Json(GetShortcutsRsp { shortcuts })
}

pub async fn add_shortcut_handler(Json(req): Json<AddShortcutReq>) -> impl IntoResponse {
    let mut shortcuts = load_shortcuts().await.unwrap_or_default();
    let new_shortcut = Shortcut {
        id: Uuid::new_v4().to_string(),
        keys: req.keys,
    };
    shortcuts.push(new_shortcut);
    let json = serde_json::to_string(&shortcuts).unwrap();
    let _ = tokio::fs::write(SHORTCUT_FILE, json).await;
    StatusCode::OK
}

pub async fn delete_shortcut_handler(Json(req): Json<DeleteShortcutReq>) -> impl IntoResponse {
    let mut shortcuts = load_shortcuts().await.unwrap_or_default();
    let original_len = shortcuts.len();
    shortcuts.retain(|s| s.id != req.id);
    if shortcuts.len() != original_len {
        let _ = save_shortcuts(&shortcuts).await;
        StatusCode::OK.into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

pub async fn get_hid_mode_handler() -> impl IntoResponse {
    match tokio::fs::read_to_string(HID_MODE_FLAG).await {
        Ok(s) => {
            let mode = match s.trim() {
                "0x0510" => "normal",
                "0x0623" => "hid-only",
                _ => "unknown",
            };
            Json(GetHidModeRsp { mode: mode.to_string() }).into_response()
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Failed to read HID mode").into_response(),
    }
}

pub async fn set_hid_mode_handler(Json(req): Json<SetHidModeReq>) -> impl IntoResponse {
    let src = match req.mode.as_str() {
        "normal" => "/kvmapp/system/init.d/S03usbdev",
        "hid-only" => "/kvmapp/system/init.d/S03usbhid",
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let dst = "/etc/init.d/S03usbdev";
    if tokio::fs::copy(src, dst).await.is_ok() {
        let _ = std::process::Command::new("chmod").arg("755").arg(dst).status();
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = std::process::Command::new("reboot").status();
        });
        StatusCode::OK.into_response()
    } else {
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

pub async fn reset_hid_handler() -> impl IntoResponse {
    let _ = std::process::Command::new("sh").arg("-c").arg("/etc/init.d/S03usbdev restart_phy").status();
    StatusCode::OK
}

async fn load_shortcuts() -> anyhow::Result<Vec<Shortcut>> {
    if !std::path::Path::new(SHORTCUT_FILE).exists() {
        return Ok(Vec::new());
    }
    let content = tokio::fs::read_to_string(SHORTCUT_FILE).await?;
    let shortcuts: Vec<Shortcut> = serde_json::from_str(&content)?;
    Ok(shortcuts)
}

async fn save_shortcuts(shortcuts: &[Shortcut]) -> anyhow::Result<()> {
    let json = serde_json::to_string(shortcuts)?;
    tokio::fs::write(SHORTCUT_FILE, json).await?;
    Ok(())
}

fn get_char_map(lang: &str) -> std::collections::HashMap<char, (u8, u8)> {
    let mut m = std::collections::HashMap::new();
    // Default US map (subset for brevity, should be expanded)
    let chars = "abcdefghijklmnopqrstuvwxyz";
    for (i, c) in chars.chars().enumerate() {
        m.insert(c, (0, 4 + i as u8));
        m.insert(c.to_uppercase().next().unwrap(), (2, 4 + i as u8));
    }
    // Numbers
    for i in 0..9 {
        m.insert((b'1' + i as u8) as char, (0, 30 + i as u8));
    }
    m.insert('0', (0, 39));
    m.insert(' ', (0, 44));
    m.insert('\n', (0, 40));
    m.insert('\t', (0, 43));

    if lang == "de" {
        m.insert('z', (0, 28)); m.insert('Z', (2, 28));
        m.insert('y', (0, 29)); m.insert('Y', (2, 29));
    }
    m
}
