use crate::api::ApiResponse;
use crate::AppState;
use axum::{
    extract::{Json, State},
    response::{IntoResponse, Response},
};
use hid::{Shortcut, ShortcutKey};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

// New sub-module for the mouse wheel direction/speed server profile (Iteration 6).
pub mod mouse_scroll;
pub use mouse_scroll::{get_mouse_scroll_handler, set_mouse_scroll_handler};

#[derive(Debug, Deserialize)]
pub struct PasteReq {
    pub content: String,
    // Frontend sends `langue` (typo). Keep compatibility with both spellings.
    #[serde(rename = "langue", alias = "language", default)]
    pub language: Option<String>,
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
const LEADER_KEY_FILE: &str = "/etc/kvm/leader-key";
const HID_MODE_FLAG: &str = "/sys/kernel/config/usb_gadget/g0/bcdDevice";

#[derive(Debug, Deserialize)]
pub struct SetLeaderKeyReq {
    #[serde(default)]
    pub key: String,
}

#[derive(Debug, Serialize)]
pub struct GetLeaderKeyRsp {
    pub key: String,
}

pub async fn paste_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PasteReq>,
) -> impl IntoResponse {
    if req.content.len() > 1024 {
        return Json(ApiResponse::<serde_json::Value>::err(
            crate::api::error_codes::VALIDATION,
            "content too long",
        ))
        .into_response();
    }

    let char_map = get_char_map(req.language.as_deref().unwrap_or(""));
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

    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

pub async fn get_shortcuts_handler() -> impl IntoResponse {
    let shortcuts = load_shortcuts().await.unwrap_or_default();
    Json(ApiResponse::ok(GetShortcutsRsp { shortcuts }))
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
    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

pub async fn delete_shortcut_handler(Json(req): Json<DeleteShortcutReq>) -> impl IntoResponse {
    let mut shortcuts = load_shortcuts().await.unwrap_or_default();
    let original_len = shortcuts.len();
    shortcuts.retain(|s| s.id != req.id);
    if shortcuts.len() != original_len {
        let _ = save_shortcuts(&shortcuts).await;
        Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
    } else {
        Json(ApiResponse::<serde_json::Value>::err(
            crate::api::error_codes::NOT_FOUND,
            "not found",
        ))
        .into_response()
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
            Json(ApiResponse::ok(GetHidModeRsp {
                mode: mode.to_string(),
            }))
            .into_response()
        }
        Err(e) => Json(ApiResponse::<GetHidModeRsp>::err(
            -1,
            &format!("failed: {}", e),
        ))
        .into_response(),
    }
}

pub async fn set_hid_mode_handler(Json(req): Json<SetHidModeReq>) -> impl IntoResponse {
    let src = match req.mode.as_str() {
        "normal" => "/kvmapp/system/init.d/S03usbdev",
        "hid-only" => "/kvmapp/system/init.d/S03usbhid",
        _ => {
            return Json(ApiResponse::<serde_json::Value>::err(
                -1,
                "invalid arguments",
            ))
            .into_response()
        }
    };

    let dst = "/etc/init.d/S03usbdev";
    if tokio::fs::copy(src, dst).await.is_ok() {
        let _ = std::process::Command::new("chmod")
            .arg("755")
            .arg(dst)
            .status();
        tokio::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let _ = std::process::Command::new("reboot").status();
        });
        Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
    } else {
        Json(ApiResponse::<serde_json::Value>::err(
            crate::api::error_codes::GENERIC,
            "failed",
        ))
        .into_response()
    }
}

pub async fn reset_hid_handler() -> impl IntoResponse {
    let _ = std::process::Command::new("sh")
        .arg("-c")
        .arg("/etc/init.d/S03usbdev restart_phy")
        .status();
    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

pub async fn set_leader_key_handler(Json(req): Json<SetLeaderKeyReq>) -> Response {
    let key = req.key.trim().to_string();
    if key.is_empty() {
        match tokio::fs::remove_file(LEADER_KEY_FILE).await {
            Ok(()) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
            }
            Err(e) => Json(ApiResponse::<serde_json::Value>::err(
                -1,
                &format!("reset failed: {e}"),
            ))
            .into_response(),
        }
    } else {
        match tokio::fs::write(LEADER_KEY_FILE, key).await {
            Ok(()) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
            Err(e) => Json(ApiResponse::<serde_json::Value>::err(
                -1,
                &format!("write failed: {e}"),
            ))
            .into_response(),
        }
    }
}

pub async fn get_leader_key_handler() -> Response {
    match tokio::fs::read_to_string(LEADER_KEY_FILE).await {
        Ok(s) => Json(ApiResponse::ok(GetLeaderKeyRsp {
            key: s.trim().to_string(),
        }))
        .into_response(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Json(ApiResponse::ok(GetLeaderKeyRsp {
                key: "".to_string(),
            }))
            .into_response()
        }
        Err(e) => Json(ApiResponse::<GetLeaderKeyRsp>::err(
            -1,
            &format!("read failed: {e}"),
        ))
        .into_response(),
    }
}

#[derive(Serialize, Deserialize)]
struct ShortcutStore {
    shortcuts: Vec<Shortcut>,
}

async fn load_shortcuts() -> anyhow::Result<Vec<Shortcut>> {
    if !std::path::Path::new(SHORTCUT_FILE).exists() {
        return Ok(Vec::new());
    }
    let content = tokio::fs::read_to_string(SHORTCUT_FILE).await?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }

    // Go (>=2.3.2) stores {"shortcuts":[...]}.
    // Older Rust builds stored a raw JSON array.
    if let Ok(store) = serde_json::from_str::<ShortcutStore>(&content) {
        return Ok(store.shortcuts);
    }
    Ok(serde_json::from_str::<Vec<Shortcut>>(&content)?)
}

async fn save_shortcuts(shortcuts: &[Shortcut]) -> anyhow::Result<()> {
    // Match Go's on-disk format for compatibility across upgrades.
    let json = serde_json::to_string(&ShortcutStore {
        shortcuts: shortcuts.to_vec(),
    })?;
    tokio::fs::write(SHORTCUT_FILE, json).await?;
    Ok(())
}

// Mouse wheel direction/speed full support (including HID report changes) is documented
// in IMPLEMENTATION_PLAN.md. No partial/stub implementations will be shipped.

fn get_char_map(lang: &str) -> std::collections::HashMap<char, (u8, u8)> {
    let mut m = std::collections::HashMap::new();

    // Base US Map
    let base_chars = [
        ('a', 0, 4),
        ('b', 0, 5),
        ('c', 0, 6),
        ('d', 0, 7),
        ('e', 0, 8),
        ('f', 0, 9),
        ('g', 0, 10),
        ('h', 0, 11),
        ('i', 0, 12),
        ('j', 0, 13),
        ('k', 0, 14),
        ('l', 0, 15),
        ('m', 0, 16),
        ('n', 0, 17),
        ('o', 0, 18),
        ('p', 0, 19),
        ('q', 0, 20),
        ('r', 0, 21),
        ('s', 0, 22),
        ('t', 0, 23),
        ('u', 0, 24),
        ('v', 0, 25),
        ('w', 0, 26),
        ('x', 0, 27),
        ('y', 0, 28),
        ('z', 0, 29),
        ('A', 2, 4),
        ('B', 2, 5),
        ('C', 2, 6),
        ('D', 2, 7),
        ('E', 2, 8),
        ('F', 2, 9),
        ('G', 2, 10),
        ('H', 2, 11),
        ('I', 2, 12),
        ('J', 2, 13),
        ('K', 2, 14),
        ('L', 2, 15),
        ('M', 2, 16),
        ('N', 2, 17),
        ('O', 2, 18),
        ('P', 2, 19),
        ('Q', 2, 20),
        ('R', 2, 21),
        ('S', 2, 22),
        ('T', 2, 23),
        ('U', 2, 24),
        ('V', 2, 25),
        ('W', 2, 26),
        ('X', 2, 27),
        ('Y', 2, 28),
        ('Z', 2, 29),
        ('1', 0, 30),
        ('2', 0, 31),
        ('3', 0, 32),
        ('4', 0, 33),
        ('5', 0, 34),
        ('6', 0, 35),
        ('7', 0, 36),
        ('8', 0, 37),
        ('9', 0, 38),
        ('0', 0, 39),
        ('!', 2, 30),
        ('@', 2, 31),
        ('#', 2, 32),
        ('$', 2, 33),
        ('%', 2, 34),
        ('^', 2, 35),
        ('&', 2, 36),
        ('*', 2, 37),
        ('(', 2, 38),
        (')', 2, 39),
        ('\n', 0, 40),
        ('\t', 0, 43),
        (' ', 0, 44),
        ('-', 0, 45),
        ('=', 0, 46),
        ('[', 0, 47),
        (']', 0, 48),
        ('\\', 0, 49),
        (';', 0, 51),
        ('\'', 0, 52),
        ('`', 0, 53),
        (',', 0, 54),
        ('.', 0, 55),
        ('/', 0, 56),
        ('_', 2, 45),
        ('+', 2, 46),
        ('{', 2, 47),
        ('}', 2, 48),
        ('|', 2, 49),
        (':', 2, 51),
        ('"', 2, 52),
        ('~', 2, 53),
        ('<', 2, 54),
        ('>', 2, 55),
        ('?', 2, 56),
    ];

    for (c, mods, code) in base_chars {
        m.insert(c, (mods, code));
    }

    if lang == "de" {
        m.insert('y', (0, 29));
        m.insert('Y', (2, 29));
        m.insert('z', (0, 28));
        m.insert('Z', (2, 28));
        m.insert('ä', (0, 52));
        m.insert('Ä', (2, 52));
        m.insert('ö', (0, 51));
        m.insert('Ö', (2, 51));
        m.insert('ü', (0, 47));
        m.insert('Ü', (2, 47));
        m.insert('ß', (0, 45));
        m.insert('^', (0, 53));
        m.insert('/', (2, 36));
        m.insert('(', (2, 37));
        m.insert('&', (2, 35));
        m.insert(')', (2, 38));
        m.insert('`', (2, 46));
        m.insert('"', (2, 31));
        m.insert('?', (2, 45));
        m.insert('{', (0x40, 36));
        m.insert('[', (0x40, 37));
        m.insert(']', (0x40, 38));
        m.insert('}', (0x40, 39));
        m.insert('\\', (0x40, 45));
        m.insert('@', (0x40, 20));
        m.insert('+', (0, 48));
        m.insert('*', (2, 48));
        m.insert('~', (0x40, 48));
        m.insert('#', (0, 49));
        m.insert('\'', (2, 49));
        m.insert('<', (0, 100));
        m.insert('>', (2, 100));
        m.insert('|', (0x40, 100));
        m.insert(';', (2, 54));
        m.insert(':', (2, 55));
        m.insert('-', (0, 56));
        m.insert('_', (2, 56));
        m.insert('´', (0, 46));
        m.insert('°', (2, 53));
        m.insert('§', (2, 32));
        m.insert('€', (0x40, 8));
        m.insert('²', (0x40, 31));
        m.insert('³', (0x40, 32));
    } else if lang == "fr" {
        // French AZERTY (matching Go implementation)
        m.insert('a', (0, 20));
        m.insert('A', (2, 20));
        m.insert('q', (0, 4));
        m.insert('Q', (2, 4));
        m.insert('z', (0, 26));
        m.insert('Z', (2, 26));
        m.insert('w', (0, 29));
        m.insert('W', (2, 29));
        m.insert('m', (0, 51));
        m.insert('M', (2, 51));

        // Numbers require Shift on AZERTY
        m.insert('1', (2, 30));
        m.insert('2', (2, 31));
        m.insert('3', (2, 32));
        m.insert('4', (2, 33));
        m.insert('5', (2, 34));
        m.insert('6', (2, 35));
        m.insert('7', (2, 36));
        m.insert('8', (2, 37));
        m.insert('9', (2, 38));
        m.insert('0', (2, 39));

        // Unshifted number row
        m.insert('&', (0, 30));
        m.insert('é', (0, 31));
        m.insert('"', (0, 32));
        m.insert('\'', (0, 33));
        m.insert('(', (0, 34));
        m.insert('-', (0, 35));
        m.insert('è', (0, 36));
        m.insert('_', (0, 37));
        m.insert('ç', (0, 38));
        m.insert('à', (0, 39));

        // Other French characters
        m.insert(')', (0, 45));
        m.insert('°', (2, 45));
        m.insert('^', (0, 47));
        m.insert('¨', (2, 47));
        m.insert('$', (0, 48));
        m.insert('£', (2, 48));
        m.insert('*', (0, 49));
        m.insert('µ', (2, 49));
        m.insert('ù', (0, 52));
        m.insert('%', (2, 52));
        m.insert(',', (0, 16));
        m.insert('?', (2, 16));
        m.insert(';', (0, 54));
        m.insert('.', (2, 54));
        m.insert(':', (0, 55));
        m.insert('/', (2, 55));
        m.insert('!', (0, 56));
        m.insert('§', (2, 56));

        // AltGr combinations
        m.insert('~', (0x40, 31));
        m.insert('#', (0x40, 32));
        m.insert('{', (0x40, 33));
        m.insert('[', (0x40, 34));
        m.insert('|', (0x40, 35));
        m.insert('`', (0x40, 36));
        m.insert('\\', (0x40, 37));
        m.insert(']', (0x40, 45));
        m.insert('}', (0x40, 46));
        m.insert('@', (0x40, 39));
        m.insert('€', (0x40, 8));
    } else if lang == "de" {
        // German QWERTZ layout (matching Go implementation)
        // Y/Z swap
        m.insert('y', (0, 29));
        m.insert('Y', (2, 29));
        m.insert('z', (0, 28));
        m.insert('Z', (2, 28));

        // German umlauts and ß
        m.insert('ä', (0, 52));
        m.insert('Ä', (2, 52));
        m.insert('ö', (0, 51));
        m.insert('Ö', (2, 51));
        m.insert('ü', (0, 47));
        m.insert('Ü', (2, 47));
        m.insert('ß', (0, 45));

        // Special character remappings (German layout)
        m.insert('^', (0, 53)); // must be double
        m.insert('/', (2, 36)); // Shift + 7
        m.insert('(', (2, 37)); // Shift + 8
        m.insert('&', (2, 35)); // Shift + 6
        m.insert(')', (2, 38)); // Shift + 9
        m.insert('`', (2, 46)); // Grave Accent / Backtick
        m.insert('"', (2, 31)); // Shift + 2
        m.insert('?', (2, 45)); // Shift + ß
        m.insert('{', (0x40, 36)); // AltGr + 7
        m.insert('[', (0x40, 37)); // AltGr + 8
        m.insert(']', (0x40, 38)); // AltGr + 6
        m.insert('}', (0x40, 39)); // AltGr + 0
        m.insert('\\', (0x40, 45)); // AltGr + ß
        m.insert('@', (0x40, 20)); // AltGr + q
        m.insert('+', (0, 48)); // Shift + +
        m.insert('*', (2, 48)); // Shift + +
        m.insert('~', (0x40, 48)); // Shift + +
        m.insert('#', (0, 49)); // Shift + #
        m.insert('\'', (2, 49)); // Shift + #
        m.insert('<', (0, 100)); // Shift + <
        m.insert('>', (2, 100)); // Shift + <
        m.insert('|', (0x40, 100)); // AltGr + <
        m.insert(';', (2, 54)); // Shift + ,
        m.insert(':', (2, 55)); // Shift + .
        m.insert('-', (0, 56)); // Shift + -
        m.insert('_', (2, 56)); // Shift + -

        // Additional German special characters
        m.insert('´', (0, 46));
        m.insert('°', (2, 53));
        m.insert('§', (2, 32));
        m.insert('€', (0x40, 8));
        m.insert('²', (0x40, 31));
        m.insert('³', (0x40, 32));
    }

    m
}
