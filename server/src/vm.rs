#![allow(dead_code)]

use crate::api::ApiResponse;
use crate::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Json, Multipart, Path, State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
#[cfg(target_os = "linux")]
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::process::Command;
use tracing::{error, info, warn};

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;

#[cfg(target_os = "linux")]
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

#[derive(Debug, Deserialize)]
pub struct SetGpioReq {
    #[serde(rename = "type", alias = "Type")]
    pub gpio_type: String, // reset / power
    #[serde(rename = "duration", alias = "Duration", default = "default_duration")]
    pub duration: u64, // press time (unit: milliseconds)
}

fn default_duration() -> u64 {
    800
}

#[derive(Debug, Serialize)]
pub struct GetGpioRsp {
    pub pwr: bool, // power led
    pub hdd: bool, // hdd led
}

#[derive(Debug, Serialize)]
pub struct GetMouseJigglerRsp {
    pub enabled: bool,
    pub mode: String,
}

#[derive(Debug, Deserialize)]
pub struct SetMouseJigglerReq {
    pub enabled: bool,
    pub mode: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IP {
    pub name: String,
    pub addr: String,
    pub version: String,
    #[serde(rename = "type")]
    pub ip_type: String,
}

#[derive(Debug, Serialize)]
pub struct GetInfoRsp {
    pub ips: Vec<IP>,
    pub mdns: String,
    pub image: String,
    pub application: String,
    #[serde(rename = "deviceKey")]
    pub device_key: String,
}

#[derive(Debug, Serialize)]
pub struct GetHardwareRsp {
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct SetOledReq {
    pub sleep: i32,
}

#[derive(Debug, Serialize)]
pub struct GetOLEDRsp {
    pub exist: bool,
    pub sleep: i32,
}

#[derive(Debug, Serialize)]
pub struct GetHdmiStateRsp {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct WinSize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Serialize)]
pub struct GetScriptsRsp {
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct UploadScriptRsp {
    pub file: String,
}

#[derive(Debug, Deserialize)]
pub struct RunScriptReq {
    pub name: String,
    #[serde(rename = "type", alias = "Type")]
    pub script_type: String, // foreground | background
}

#[derive(Debug, Serialize)]
pub struct RunScriptRsp {
    pub log: String,
}

#[derive(Debug, Deserialize)]
pub struct DeleteScriptReq {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct GetVirtualDeviceRsp {
    pub network: bool,
    pub disk: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateVirtualDeviceReq {
    pub device: String,
}

#[derive(Debug, Serialize)]
pub struct UpdateVirtualDeviceRsp {
    pub on: bool,
}

#[derive(Debug, Serialize)]
pub struct GetMdnsStateRsp {
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct GetSSHStateRsp {
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
pub struct GetSwapRsp {
    pub size: i64, // unit: MB
}

#[derive(Debug, Deserialize)]
pub struct SetSwapReq {
    pub size: i64,
}

#[derive(Debug, Serialize)]
pub struct GetHostnameRsp {
    pub hostname: String,
}

#[derive(Debug, Deserialize)]
pub struct SetHostnameReq {
    pub hostname: String,
}

#[derive(Debug, Serialize)]
pub struct GetWebTitleRsp {
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct SetWebTitleReq {
    pub title: String,
}

#[derive(Debug, Serialize)]
pub struct GetAutostartRsp {
    pub files: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UploadAutostartReq {
    pub content: String,
}

const OLED_EXIST_FILE: &str = "/etc/kvm/oled_exist";
const OLED_SLEEP_FILE: &str = "/etc/kvm/oled_sleep";
const SCRIPT_DIRECTORY: &str = "/etc/kvm/scripts";
const GOMEMLIMIT_FILE: &str = "/etc/kvm/GOMEMLIMIT";

const VIRTUAL_NETWORK: &str = "/boot/usb.rndis0";
const VIRTUAL_DISK: &str = "/boot/usb.disk0";

const AVAHI_PID_FILE: &str = "/run/avahi-daemon/pid";
const AVAHI_SCRIPT: &str = "/etc/init.d/S50avahi-daemon";
const AVAHI_BACKUP_SCRIPT: &str = "/kvmapp/system/init.d/S50avahi-daemon";

const SSH_SCRIPT: &str = "/etc/init.d/S50sshd";
const SSH_STOP_FLAG: &str = "/etc/kvm/ssh_stop";

const SWAP_FILE: &str = "/swapfile";
const ETC_HOSTNAME: &str = "/etc/hostname";
const ETC_HOSTS: &str = "/etc/hosts";
const BOOT_HOSTNAME: &str = "/boot/hostname";
const WEB_TITLE_FILE: &str = "/etc/kvm/web-title";
const AUTOSTART_DIRECTORY: &str = "/etc/kvm/autostart";
const HDMI_DISABLE_FILE: &str = "/etc/kvm/hdmi_disable";
const INITTAB_PATH: &str = "/etc/inittab";

#[derive(Debug, Serialize)]
pub struct GetMemoryLimitRsp {
    pub enabled: bool,
    pub limit: i64,
}

#[derive(Debug, Deserialize)]
pub struct SetMemoryLimitReq {
    pub enabled: bool,
    pub limit: i64,
}

fn sanitize_file_name(file_name: &str) -> Option<String> {
    let sanitized: String = file_name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-' || *c == '.' || *c == ' ')
        .collect();

    if sanitized.is_empty() || sanitized.contains("..") {
        return None;
    }

    Some(sanitized)
}

fn validate_script_path(base_dir: &str, file_name: &str) -> Option<std::path::PathBuf> {
    let sanitized = sanitize_file_name(file_name)?;
    let base_path = std::path::Path::new(base_dir);
    let full_path = base_path.join(&sanitized);

    if full_path.starts_with(base_path) {
        Some(full_path)
    } else {
        None
    }
}

async fn run_shell_command(cmd: &str) -> bool {
    match Command::new("sh").arg("-c").arg(cmd).status().await {
        Ok(status) => status.success(),
        Err(e) => {
            warn!("Failed to execute command '{}': {}", cmd, e);
            false
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SetScreenReq {
    #[serde(rename = "type", alias = "Type")]
    pub screen_type: String, // resolution / fps / quality / type / gop
    #[serde(rename = "value", alias = "Value")]
    pub value: i32,
}

const SCREEN_TYPE_FILE: &str = "/kvmapp/kvm/type";
const SCREEN_FPS_FILE: &str = "/kvmapp/kvm/fps";
const SCREEN_QUALITY_FILE: &str = "/kvmapp/kvm/qlty";
const SCREEN_RES_FILE: &str = "/kvmapp/kvm/res";

#[cfg(target_os = "linux")]
pub async fn set_screen_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetScreenReq>,
) -> impl IntoResponse {
    // Handle file writes outside the lock to avoid holding lock across await
    let file_write: Option<(&str, String)> = match req.screen_type.as_str() {
        "type" => {
            let stream_type = if req.value == 0 { "mjpeg" } else { "h264" };
            Some((SCREEN_TYPE_FILE, stream_type.to_string()))
        }
        "fps" => Some((SCREEN_FPS_FILE, req.value.to_string())),
        "quality" => Some((SCREEN_QUALITY_FILE, req.value.to_string())),
        "resolution" => Some((SCREEN_RES_FILE, req.value.to_string())),
        "gop" => None,
        _ => {
            return Json(ApiResponse::<serde_json::Value>::err(
                -1,
                "invalid arguments",
            ))
            .into_response();
        }
    };

    // Do file write without holding the lock
    if let Some((path, content)) = file_write {
        let _ = tokio::fs::write(path, content).await;
    }

    // Now update the in-memory config
    {
        let mut config = state.screen_config.write();
        match req.screen_type.as_str() {
            "type" => {
                config.stream_type = if req.value == 0 {
                    "mjpeg".to_string()
                } else {
                    "h264".to_string()
                };
            }
            "fps" => {
                let fps = req.value.clamp(1, 60) as u16;
                config.fps = fps;
            }
            "quality" => {
                // UI sends 50-100 for MJPEG, and 1000-5000 for H.264 bitrate.
                if req.value <= 100 {
                    config.quality = req.value.clamp(1, 100) as u16;
                } else {
                    config.bitrate = req.value.clamp(100, 20_000) as u16;
                }
            }
            "gop" => {
                config.gop = req.value.clamp(1, 100) as u8;
                state.kvm.set_h264_gop(config.gop);
            }
            "resolution" => {
                // Frontend sends the height as the value:
                // 0(auto), 1080, 720, 600, 480.
                // Keep a small compatibility shim for older index-based values.
                let (w, h) = match req.value {
                    0 => (0, 0),
                    1080 | 1 => (1920, 1080),
                    720 | 2 => (1280, 720),
                    600 | 3 => (800, 600),
                    480 | 4 => (640, 480),
                    _ => (config.width, config.height),
                };
                config.width = w;
                config.height = h;
            }
            _ => {}
        }
    }

    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

pub async fn get_memory_limit_handler() -> impl IntoResponse {
    let exist = std::path::Path::new(GOMEMLIMIT_FILE).exists();
    if !exist {
        return Json(ApiResponse::ok(GetMemoryLimitRsp {
            enabled: false,
            limit: 0,
        }))
        .into_response();
    }

    match tokio::fs::read_to_string(GOMEMLIMIT_FILE).await {
        Ok(content) => {
            let limit = content.trim().parse::<i64>().unwrap_or(0);
            Json(ApiResponse::ok(GetMemoryLimitRsp {
                enabled: true,
                limit,
            }))
            .into_response()
        }
        Err(e) => Json(ApiResponse::<GetMemoryLimitRsp>::err(
            -1,
            &format!("failed to read memory limit: {}", e),
        ))
        .into_response(),
    }
}

pub async fn set_memory_limit_handler(Json(req): Json<SetMemoryLimitReq>) -> impl IntoResponse {
    if req.enabled {
        let limit = req.limit.max(50);
        let _ = tokio::fs::create_dir_all("/etc/kvm").await;
        if let Err(e) = tokio::fs::write(GOMEMLIMIT_FILE, limit.to_string()).await {
            return Json(ApiResponse::<serde_json::Value>::err(
                -2,
                &format!("failed to set memory limit: {}", e),
            ))
            .into_response();
        }
    } else {
        let _ = tokio::fs::remove_file(GOMEMLIMIT_FILE).await;
    }

    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

#[derive(Debug, Deserialize)]
pub struct SetTlsReq {
    pub enabled: bool,
}

pub async fn set_tls_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetTlsReq>,
) -> impl IntoResponse {
    let mut config = (*state.config).clone();
    config.proto = if req.enabled {
        "https".to_string()
    } else {
        "http".to_string()
    };

    if req.enabled {
        config.cert.crt = "/etc/kvm/server.crt".to_string();
        config.cert.key = "/etc/kvm/server.key".to_string();
        // Generate cert if missing? Go uses utils.GenerateCert()
        let _ = Command::new("sh")
            .arg("-c")
            .arg("/kvmapp/system/gen-cert.sh")
            .status()
            .await;
    }

    match config.save().await {
        Ok(_) => {
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = Command::new("sh")
                    .arg("-c")
                    .arg("/etc/init.d/S95nanokvm restart")
                    .status()
                    .await;
            });
            Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, &e.to_string())).into_response(),
    }
}

pub async fn set_oled_handler(Json(req): Json<SetOledReq>) -> impl IntoResponse {
    match tokio::fs::write(OLED_SLEEP_FILE, req.sleep.to_string()).await {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, &e.to_string())).into_response(),
    }
}

pub async fn get_oled_handler() -> impl IntoResponse {
    let exist = std::path::Path::new(OLED_EXIST_FILE).exists();
    let mut sleep = 0;

    if exist {
        if let Ok(content) = tokio::fs::read_to_string(OLED_SLEEP_FILE).await {
            if let Ok(val) = content.trim().parse::<i32>() {
                sleep = val;
            }
        }
    }

    Json(ApiResponse::ok(GetOLEDRsp { exist, sleep }))
}

pub async fn get_info_handler() -> impl IntoResponse {
    let mut ips = Vec::new();
    if let Ok(ifaces) = get_if_addrs::get_if_addrs() {
        for iface in ifaces {
            if !iface.is_loopback() {
                let ip_type = if iface.name.starts_with("eth") || iface.name.starts_with("en") {
                    "Wired"
                } else if iface.name.starts_with("wlan") || iface.name.starts_with("wl") {
                    "Wireless"
                } else {
                    continue;
                };

                if let std::net::IpAddr::V4(addr) = iface.ip() {
                    ips.push(IP {
                        name: iface.name,
                        addr: addr.to_string(),
                        version: "IPv4".to_string(),
                        ip_type: ip_type.to_string(),
                    });
                }
            }
        }
    }

    let hostname = tokio::fs::read_to_string("/etc/hostname")
        .await
        .unwrap_or_default()
        .trim()
        .to_string();
    let mdns = if !hostname.is_empty() {
        format!("{}.local", hostname)
    } else {
        String::new()
    };

    let image = tokio::fs::read_to_string("/boot/ver")
        .await
        .unwrap_or_default()
        .trim()
        .to_string();
    let application = tokio::fs::read_to_string("/kvmapp/version")
        .await
        .unwrap_or_else(|_| "1.0.0".to_string())
        .trim()
        .to_string();
    let device_key = tokio::fs::read_to_string("/device_key")
        .await
        .unwrap_or_default()
        .trim()
        .to_string();

    Json(ApiResponse::ok(GetInfoRsp {
        ips,
        mdns,
        image,
        application,
        device_key,
    }))
}

#[cfg(target_os = "linux")]
pub async fn get_hardware_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let version = match state.vm.get_version() {
        vm::HardwareVersion::Alpha => "Alpha",
        vm::HardwareVersion::Beta => "Beta",
        vm::HardwareVersion::Pcie => "PCIE",
    };

    Json(ApiResponse::ok(GetHardwareRsp {
        version: version.to_string(),
    }))
}

// EDID and serial terminal full support are documented in IMPLEMENTATION_PLAN.md
// under the "Hardware-Dependent Features Requiring New FFI" section.
// No partial implementations will be added until complete logic + FFI bindings are ready.

#[cfg(target_os = "linux")]
pub async fn set_gpio_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetGpioReq>,
) -> impl IntoResponse {
    info!("Set GPIO request: {:?}", req);

    let result = match req.gpio_type.as_str() {
        "power" => state.vm.power_press(req.duration).await,
        "reset" => state.vm.reset_press(req.duration).await,
        _ => {
            return Json(ApiResponse::<serde_json::Value>::err(
                -1,
                "invalid gpio type",
            ))
            .into_response();
        }
    };

    match result {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => {
            error!("GPIO control failed: {}", e);
            Json(ApiResponse::<serde_json::Value>::err(
                -1,
                &format!("gpio error: {}", e),
            ))
            .into_response()
        }
    }
}

#[cfg(target_os = "linux")]
pub async fn get_gpio_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pwr = state.vm.get_power_led().await.unwrap_or(false);
    let hdd = state.vm.get_hdd_led().await.unwrap_or(false);

    Json(ApiResponse::ok(GetGpioRsp { pwr, hdd }))
}

#[cfg(target_os = "linux")]
pub async fn get_jiggler_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let enabled = state.jiggler.is_enabled().await;
    let mode = state.jiggler.get_mode().await;

    Json(ApiResponse::ok(GetMouseJigglerRsp { enabled, mode }))
}

#[cfg(target_os = "linux")]
pub async fn set_jiggler_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetMouseJigglerReq>,
) -> impl IntoResponse {
    let res: Result<(), anyhow::Error> = if req.enabled {
        state
            .jiggler
            .enable(req.mode.as_deref().unwrap_or("relative"))
            .await
            .map_err(|e| anyhow::anyhow!(e))
    } else {
        state
            .jiggler
            .disable()
            .await
            .map_err(|e| anyhow::anyhow!(e))
    };

    match res {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, &e.to_string())).into_response(),
    }
}

pub async fn terminal_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_terminal_socket)
}

#[allow(unused_mut)]
async fn handle_terminal_socket(mut socket: WebSocket) {
    #[cfg(target_os = "linux")]
    {
        let pty_system = native_pty_system();
        let pair = match pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(p) => p,
            Err(e) => {
                error!("Failed to open pty: {}", e);
                return;
            }
        };

        let cmd = CommandBuilder::new("/bin/sh");
        let mut child = match pair.slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => {
                error!("Failed to spawn shell: {}", e);
                return;
            }
        };

        let reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                error!("Failed to clone PTY reader: {}", e);
                let _ = child.kill();
                return;
            }
        };
        let mut writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to get PTY writer: {}", e);
                let _ = child.kill();
                return;
            }
        };

        let (mut ws_sender, mut ws_receiver) = socket.split();

        let reader = std::sync::Arc::new(std::sync::Mutex::new(reader));
        let mut reader_task = tokio::spawn(async move {
            loop {
                let reader_clone = reader.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let mut b = [0u8; 1024];
                    let mut guard = reader_clone.lock().unwrap();
                    match guard.read(&mut b) {
                        Ok(n) => (b, n),
                        Err(_) => (b, 0),
                    }
                })
                .await
                .unwrap();

                if result.1 > 0 {
                    if ws_sender
                        .send(Message::Binary(result.0[..result.1].to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                } else {
                    break;
                }
            }
        });

        let mut writer_task = tokio::spawn(async move {
            while let Some(Ok(msg)) = ws_receiver.next().await {
                match msg {
                    Message::Binary(bin) => {
                        if let Ok(win_size) = serde_json::from_slice::<WinSize>(&bin) {
                            let _ = pair.master.resize(PtySize {
                                rows: win_size.rows,
                                cols: win_size.cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            });
                        } else {
                            let _ = writer.write_all(&bin);
                        }
                    }
                    Message::Text(text) => {
                        let _ = writer.write_all(text.as_bytes());
                    }
                    _ => {}
                }
            }
        });

        tokio::select! {
            _ = &mut reader_task => { writer_task.abort(); },
            _ = &mut writer_task => { reader_task.abort(); },
        }

        let _ = child.kill();
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = socket
            .send(Message::Text("Terminal only supported on Linux".into()))
            .await;
    }
}

pub async fn get_scripts_handler() -> impl IntoResponse {
    let mut files = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(SCRIPT_DIRECTORY).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let name_lower = name.to_lowercase();
            if name_lower.ends_with(".sh") || name_lower.ends_with(".py") {
                files.push(name);
            }
        }
    }
    Json(ApiResponse::ok(GetScriptsRsp { files }))
}

pub async fn upload_script_handler(mut multipart: Multipart) -> impl IntoResponse {
    let _ = tokio::fs::create_dir_all(SCRIPT_DIRECTORY).await;
    let mut uploaded_file: Option<String> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if let Some(file_name) = field.file_name() {
            let name_lower = file_name.to_lowercase();
            if name_lower.ends_with(".sh") || name_lower.ends_with(".py") {
                if let Some(path) = validate_script_path(SCRIPT_DIRECTORY, file_name) {
                    if let Ok(data) = field.bytes().await {
                        if tokio::fs::write(&path, data).await.is_ok() {
                            uploaded_file = Some(
                                path.file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                            );
                        }
                        #[cfg(target_os = "linux")]
                        let _ =
                            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
                    }
                }
            }
        }
    }
    match uploaded_file {
        Some(file) => Json(ApiResponse::ok(UploadScriptRsp { file })).into_response(),
        None => Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "no file uploaded",
        ))
        .into_response(),
    }
}

pub async fn run_script_handler(Json(req): Json<RunScriptReq>) -> impl IntoResponse {
    let Some(path) = validate_script_path(SCRIPT_DIRECTORY, &req.name) else {
        return Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "invalid arguments",
        ))
        .into_response();
    };
    if !path.exists() {
        return Json(ApiResponse::<serde_json::Value>::err(
            -2,
            "script not found",
        ))
        .into_response();
    }

    if req.script_type == "foreground" {
        let output = if req.name.to_lowercase().ends_with(".py") {
            Command::new("python").arg(&path).output().await
        } else {
            Command::new("sh").arg(&path).output().await
        };

        match output {
            Ok(output) => {
                let log = String::from_utf8_lossy(&output.stdout).to_string()
                    + &String::from_utf8_lossy(&output.stderr);
                Json(ApiResponse::ok(RunScriptRsp { log })).into_response()
            }
            Err(e) => {
                Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, &e.to_string())).into_response()
            }
        }
    } else {
        let mut cmd = if req.name.to_lowercase().ends_with(".py") {
            Command::new("python")
        } else {
            Command::new("sh")
        };
        cmd.arg(&path);
        tokio::spawn(async move {
            let _ = cmd.status().await;
        });
        Json(ApiResponse::ok(RunScriptRsp {
            log: "Started in background".to_string(),
        }))
        .into_response()
    }
}

pub async fn delete_script_handler(Json(req): Json<DeleteScriptReq>) -> impl IntoResponse {
    let Some(path) = validate_script_path(SCRIPT_DIRECTORY, &req.name) else {
        return Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "invalid arguments",
        ))
        .into_response();
    };
    match tokio::fs::remove_file(path).await {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(e) => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, &e.to_string())).into_response(),
    }
}

pub async fn get_virtual_device_handler() -> impl IntoResponse {
    let network = std::path::Path::new(VIRTUAL_NETWORK).exists();
    let disk = std::path::Path::new(VIRTUAL_DISK).exists();
    Json(ApiResponse::ok(GetVirtualDeviceRsp { network, disk }))
}

pub async fn update_virtual_device_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateVirtualDeviceReq>,
) -> impl IntoResponse {
    let (device_path, mount_cmds, unmount_commands) = match req.device.as_str() {
        "network" => (
            VIRTUAL_NETWORK,
            vec![
                "touch /boot/usb.rndis0",
                "/etc/init.d/S03usbdev stop",
                "/etc/init.d/S03usbdev start",
            ],
            vec![
                "/etc/init.d/S03usbdev stop",
                "rm -rf /sys/kernel/config/usb_gadget/g0/configs/c.1/rndis.usb0",
                "rm /boot/usb.rndis0",
                "/etc/init.d/S03usbdev start",
            ],
        ),
        "disk" => (
            VIRTUAL_DISK,
            vec![
                "touch /boot/usb.disk0",
                "/etc/init.d/S03usbdev stop",
                "/etc/init.d/S03usbdev start",
            ],
            vec![
                "/etc/init.d/S03usbdev stop",
                "rm -rf /sys/kernel/config/usb_gadget/g0/configs/c.1/mass_storage.disk0",
                "rm /boot/usb.disk0",
                "/etc/init.d/S03usbdev start",
            ],
        ),
        _ => {
            return Json(ApiResponse::<serde_json::Value>::err(
                -1,
                "invalid arguments",
            ))
            .into_response()
        }
    };
    let exists = std::path::Path::new(device_path).exists();
    let cmds = if !exists {
        mount_cmds
    } else {
        unmount_commands
    };
    {
        let _hid = state.hid.lock().await;
        for cmd in cmds {
            let _ = Command::new("sh").arg("-c").arg(cmd).status().await;
        }
    }
    let on = std::path::Path::new(device_path).exists();
    Json(ApiResponse::ok(UpdateVirtualDeviceRsp { on })).into_response()
}

pub async fn get_mdns_handler() -> impl IntoResponse {
    let enabled = std::path::Path::new(AVAHI_PID_FILE).exists();
    Json(ApiResponse::ok(GetMdnsStateRsp { enabled }))
}

pub async fn enable_mdns_handler() -> impl IntoResponse {
    let cmd = format!(
        "cp -f {} {} && {} restart",
        AVAHI_BACKUP_SCRIPT, AVAHI_SCRIPT, AVAHI_SCRIPT
    );
    match Command::new("sh").arg("-c").arg(cmd).status().await {
        Ok(s) if s.success() => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        _ => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, "failed")).into_response(),
    }
}

pub async fn disable_mdns_handler() -> impl IntoResponse {
    if let Ok(pid) = tokio::fs::read_to_string(AVAHI_PID_FILE).await {
        let cmd = format!(
            "kill -9 {} && rm -f {} {}",
            pid.trim(),
            AVAHI_PID_FILE,
            AVAHI_SCRIPT
        );
        match Command::new("sh").arg("-c").arg(cmd).status().await {
            Ok(s) if s.success() => {
                Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
            }
            _ => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, "failed")).into_response(),
        }
    } else {
        Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
    }
}

pub async fn get_ssh_handler() -> impl IntoResponse {
    let enabled = !std::path::Path::new(SSH_STOP_FLAG).exists();
    Json(ApiResponse::ok(GetSSHStateRsp { enabled }))
}

pub async fn enable_ssh_handler() -> impl IntoResponse {
    match Command::new("sh")
        .arg("-c")
        .arg(format!("{} permanent_on", SSH_SCRIPT))
        .status()
        .await
    {
        Ok(s) if s.success() => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        _ => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, "failed")).into_response(),
    }
}

pub async fn disable_ssh_handler() -> impl IntoResponse {
    match Command::new("sh")
        .arg("-c")
        .arg(format!("{} permanent_off", SSH_SCRIPT))
        .status()
        .await
    {
        Ok(s) if s.success() => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        _ => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, "failed")).into_response(),
    }
}

pub async fn get_swap_handler() -> impl IntoResponse {
    let size = std::fs::metadata(SWAP_FILE)
        .map(|m| m.len() / 1024 / 1024)
        .unwrap_or(0);
    Json(ApiResponse::ok(GetSwapRsp { size: size as i64 }))
}

pub async fn set_swap_handler(Json(req): Json<SetSwapReq>) -> impl IntoResponse {
    let current_size = std::fs::metadata(SWAP_FILE)
        .map(|m| m.len() / 1024 / 1024)
        .unwrap_or(0);
    if req.size == current_size as i64 {
        return Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response();
    }

    if req.size == 0 {
        let _ = run_shell_command("swapoff -a && rm -f /swapfile").await;
        let _ = disable_inittab_swap().await;
    } else {
        if current_size > 0 {
            let _ = run_shell_command("swapoff -a && rm -f /swapfile").await;
        }
        let cmd = format!(
            "fallocate -l {}M {} && chmod 600 {} && mkswap {} && swapon {}",
            req.size, SWAP_FILE, SWAP_FILE, SWAP_FILE, SWAP_FILE
        );
        if run_shell_command(&cmd).await {
            let _ = enable_inittab_swap().await;
        }
    }
    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

async fn enable_inittab_swap() -> tokio::io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(INITTAB_PATH)
        .await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(format!("\nsi11::sysinit:/sbin/swapon {}", SWAP_FILE).as_bytes())
        .await?;
    Ok(())
}

async fn disable_inittab_swap() -> tokio::io::Result<()> {
    if let Ok(content) = tokio::fs::read_to_string(INITTAB_PATH).await {
        let new_content: Vec<&str> = content
            .lines()
            .filter(|line| !line.contains(SWAP_FILE))
            .collect();
        tokio::fs::write(INITTAB_PATH, new_content.join("\n")).await?;
    }
    Ok(())
}

pub async fn get_hostname_handler() -> impl IntoResponse {
    let hostname = tokio::fs::read_to_string(ETC_HOSTNAME)
        .await
        .unwrap_or_default()
        .trim()
        .to_string();
    Json(ApiResponse::ok(GetHostnameRsp { hostname }))
}

fn validate_hostname(hostname: &str) -> bool {
    if hostname.is_empty() || hostname.len() > 63 {
        return false;
    }

    if hostname.starts_with('.') || hostname.ends_with('.') {
        return false;
    }

    hostname
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '.')
}

pub async fn set_hostname_handler(Json(req): Json<SetHostnameReq>) -> impl IntoResponse {
    let hostname = req.hostname.trim();

    if !validate_hostname(hostname) {
        return Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "invalid hostname",
        ))
        .into_response();
    }

    if let Ok(old_hostname) = tokio::fs::read_to_string(ETC_HOSTNAME).await {
        let old_hostname = old_hostname.trim();
        if old_hostname != hostname {
            if let Ok(hosts_content) = tokio::fs::read_to_string(ETC_HOSTS).await {
                let new_hosts = hosts_content.replace(old_hostname, hostname);
                let _ = tokio::fs::write(ETC_HOSTS, new_hosts).await;
            }
        }
    }

    let _ = tokio::fs::write(ETC_HOSTNAME, hostname).await;
    let _ = tokio::fs::write(BOOT_HOSTNAME, hostname).await;
    if let Err(e) = Command::new("hostname").arg(hostname).status().await {
        warn!("Failed to set hostname: {}", e);
    }

    // Restart mDNS if enabled to reflect the new hostname
    if std::path::Path::new(AVAHI_PID_FILE).exists() {
        info!("Restarting mDNS due to hostname change to {}", hostname);
        let _ = enable_mdns_handler().await;
    }

    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

pub async fn get_web_title_handler() -> impl IntoResponse {
    let title = tokio::fs::read_to_string(WEB_TITLE_FILE)
        .await
        .unwrap_or_else(|_| "NanoKVM".to_string())
        .trim()
        .to_string();
    Json(ApiResponse::ok(GetWebTitleRsp { title }))
}

pub async fn set_web_title_handler(Json(req): Json<SetWebTitleReq>) -> impl IntoResponse {
    if req.title.is_empty() || req.title == "NanoKVM" {
        let _ = tokio::fs::remove_file(WEB_TITLE_FILE).await;
    } else {
        let _ = tokio::fs::write(WEB_TITLE_FILE, &req.title).await;
    }
    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

pub async fn get_autostart_handler() -> impl IntoResponse {
    let mut files = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(AUTOSTART_DIRECTORY).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            files.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    Json(ApiResponse::ok(GetAutostartRsp { files }))
}

pub async fn get_autostart_content_handler(Path(name): Path<String>) -> impl IntoResponse {
    let path = std::path::Path::new(AUTOSTART_DIRECTORY).join(name);
    match tokio::fs::read_to_string(path).await {
        Ok(c) => Json(ApiResponse::ok(c)).into_response(),
        Err(_) => Json(ApiResponse::<String>::err(-1, "read file fail")).into_response(),
    }
}

pub async fn upload_autostart_handler(
    Path(name): Path<String>,
    Json(req): Json<UploadAutostartReq>,
) -> impl IntoResponse {
    let _ = tokio::fs::create_dir_all(AUTOSTART_DIRECTORY).await;
    let path = std::path::Path::new(AUTOSTART_DIRECTORY).join(name);
    match tokio::fs::write(&path, &req.content).await {
        Ok(_) => {
            #[cfg(target_os = "linux")]
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
            Json(ApiResponse::ok(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
            ))
            .into_response()
        }
        Err(e) => Json(ApiResponse::<serde_json::Value>::err(crate::api::error_codes::GENERIC, &e.to_string())).into_response(),
    }
}

pub async fn delete_autostart_handler(Path(name): Path<String>) -> impl IntoResponse {
    let path = std::path::Path::new(AUTOSTART_DIRECTORY).join(name);
    match tokio::fs::remove_file(path).await {
        Ok(_) => Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response(),
        Err(_) => Json(ApiResponse::<serde_json::Value>::err(
            -1,
            "remove file fail",
        ))
        .into_response(),
    }
}

pub async fn reboot_handler() -> impl IntoResponse {
    tokio::spawn(async move {
        // Give the HTTP response a moment to flush before rebooting.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        if let Err(e) = Command::new("reboot").status().await {
            error!("Failed to execute reboot: {}", e);
        }
    });
    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

#[cfg(target_os = "linux")]
pub async fn reset_hdmi_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let _ = state.kvm.set_hdmi(false);

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let _ = state.kvm.set_hdmi(true);

    let _ = tokio::fs::remove_file(HDMI_DISABLE_FILE).await;

    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

#[cfg(target_os = "linux")]
pub async fn enable_hdmi_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let _ = state.kvm.set_hdmi(true);

    let _ = tokio::fs::remove_file(HDMI_DISABLE_FILE).await;

    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

#[cfg(target_os = "linux")]
pub async fn disable_hdmi_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let _ = state.kvm.set_hdmi(false);

    let _ = tokio::fs::write(HDMI_DISABLE_FILE, b"").await;

    Json(ApiResponse::<serde_json::Value>::ok_empty()).into_response()
}

pub async fn get_hdmi_state_handler() -> impl IntoResponse {
    let enabled = !std::path::Path::new(HDMI_DISABLE_FILE).exists();

    Json(ApiResponse::ok(GetHdmiStateRsp { enabled }))
}
