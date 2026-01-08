use axum::{
    extract::{State, Json, WebSocketUpgrade, ws::{WebSocket, Message}, Multipart, Path},
    response::IntoResponse,
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info, warn};
use crate::AppState;
use futures::{SinkExt, StreamExt};
use tokio::process::Command;

#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;

#[cfg(target_os = "linux")]
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

#[cfg(target_os = "linux")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Debug, Deserialize)]
pub struct SetGpioReq {
    #[serde(rename = "Type")]
    pub gpio_type: String, // reset / power
    #[serde(rename = "Duration", default = "default_duration")]
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

#[derive(Debug, Deserialize)]
pub struct RunScriptReq {
    pub name: String,
    #[serde(rename = "Type")]
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
    #[serde(rename = "Type")]
    pub screen_type: String, // resolution / fps / quality / type / gop
    pub value: i32,
}

const SCREEN_TYPE_FILE: &str = "/kvmapp/kvm/type";
const SCREEN_FPS_FILE: &str = "/kvmapp/kvm/fps";
const SCREEN_QUALITY_FILE: &str = "/kvmapp/kvm/qlty";
const SCREEN_RES_FILE: &str = "/kvmapp/kvm/res";

pub async fn set_screen_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetScreenReq>,
) -> impl IntoResponse {
    let mut config = state.screen_config.write();
    
    match req.screen_type.as_str() {
        "type" => {
            config.stream_type = if req.value == 0 { "mjpeg".to_string() } else { "h264".to_string() };
            let _ = tokio::fs::write(SCREEN_TYPE_FILE, &config.stream_type).await;
        }
        "fps" => {
            config.fps = req.value as u16;
            let _ = tokio::fs::write(SCREEN_FPS_FILE, config.fps.to_string()).await;
        }
        "quality" => {
            config.quality = req.value as u16;
            let _ = tokio::fs::write(SCREEN_QUALITY_FILE, config.quality.to_string()).await;
        }
        "gop" => {
            config.gop = req.value as u8;
            state.kvm.set_h264_gop(config.gop);
        }
        "resolution" => {
            // Value is usually encoded, e.g. 0 for 1280x720?
            // Go code just writes it.
            let _ = tokio::fs::write(SCREEN_RES_FILE, req.value.to_string()).await;
            match req.value {
                0 => { config.width = 1280; config.height = 720; }
                1 => { config.width = 1920; config.height = 1080; }
                2 => { config.width = 1024; config.height = 768; }
                3 => { config.width = 800; config.height = 600; }
                4 => { config.width = 640; config.height = 480; }
                _ => {}
            }
        }
        _ => return StatusCode::BAD_REQUEST.into_response(),
    }

    StatusCode::OK.into_response()
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
    config.proto = if req.enabled { "https".to_string() } else { "http".to_string() };
    
    if req.enabled {
        config.cert.crt = "/etc/kvm/server.crt".to_string();
        config.cert.key = "/etc/kvm/server.key".to_string();
        // Generate cert if missing? Go uses utils.GenerateCert()
        let _ = Command::new("sh").arg("-c").arg("/kvmapp/system/gen-cert.sh").status().await;
    }

    match config.save().await {
        Ok(_) => {
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let _ = Command::new("sh").arg("-c").arg("/etc/init.d/S95nanokvm restart").status().await;
            });
            StatusCode::OK.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn set_oled_handler(
    Json(req): Json<SetOledReq>,
) -> impl IntoResponse {
    match tokio::fs::write(OLED_SLEEP_FILE, req.sleep.to_string()).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
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

    Json(GetOLEDRsp { exist, sleep })
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

    let hostname = tokio::fs::read_to_string("/etc/hostname").await.unwrap_or_default().trim().to_string();
    let mdns = if !hostname.is_empty() { format!("{}.local", hostname) } else { String::new() };

    let image = tokio::fs::read_to_string("/boot/ver").await.unwrap_or_default().trim().to_string();
    let application = tokio::fs::read_to_string("/kvmapp/version").await.unwrap_or_else(|_| "1.0.0".to_string()).trim().to_string();
    let device_key = tokio::fs::read_to_string("/device_key").await.unwrap_or_default().trim().to_string();

    Json(GetInfoRsp {
        ips,
        mdns,
        image,
        application,
        device_key,
    })
}

#[cfg(target_os = "linux")]
pub async fn get_hardware_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let version = match state.vm.get_version() {
        vm::HardwareVersion::Alpha => "Alpha",
        vm::HardwareVersion::Beta => "Beta",
        vm::HardwareVersion::Pcie => "PCIE",
    };
    
    Json(GetHardwareRsp { version: version.to_string() })
}

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
            return (StatusCode::BAD_REQUEST, "Invalid GPIO type").into_response();
        }
    };

    match result {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => {
            error!("GPIO control failed: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("GPIO error: {}", e)).into_response()
        }
    }
}

#[cfg(target_os = "linux")]
pub async fn get_gpio_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let pwr = state.vm.get_power_led().await.unwrap_or(false);
    let hdd = state.vm.get_hdd_led().await.unwrap_or(false);
    
    Json(GetGpioRsp { pwr, hdd })
}

#[cfg(target_os = "linux")]
pub async fn get_jiggler_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let enabled = state.jiggler.is_enabled().await;
    let mode = state.jiggler.get_mode().await;
    
    Json(GetMouseJigglerRsp { enabled, mode })
}

#[cfg(target_os = "linux")]
pub async fn set_jiggler_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetMouseJigglerReq>,
) -> impl IntoResponse {
    let res = if req.enabled {
        state.jiggler.enable(req.mode.as_deref().unwrap_or("relative")).await
    } else {
        state.jiggler.disable().await
    };

    match res {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
    }
}

pub async fn terminal_handler(
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_terminal_socket(socket))
}

async fn handle_terminal_socket(mut socket: WebSocket) {
    #[cfg(target_os = "linux")]
    {
        let pty_system = native_pty_system();
        let pair = match pty_system.open_pty(PtySize {
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

        let mut reader_task = tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            loop {
                let n = tokio::task::spawn_blocking(move || {
                    let mut b = [0u8; 1024];
                    match reader.read(&mut b) {
                        Ok(n) => (b, n),
                        Err(_) => (b, 0),
                    }
                }).await.unwrap();

                if n.1 > 0 {
                    if ws_sender.send(Message::Binary(n.0[..n.1].to_vec())).await.is_err() {
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
        let _ = socket.send(Message::Text("Terminal only supported on Linux".into())).await;
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
    Json(GetScriptsRsp { files })
}

pub async fn upload_script_handler(mut multipart: Multipart) -> impl IntoResponse {
    let _ = tokio::fs::create_dir_all(SCRIPT_DIRECTORY).await;
    while let Ok(Some(field)) = multipart.next_field().await {
        if let Some(file_name) = field.file_name() {
            let name_lower = file_name.to_lowercase();
            if name_lower.ends_with(".sh") || name_lower.ends_with(".py") {
                if let Some(path) = validate_script_path(SCRIPT_DIRECTORY, file_name) {
                    if let Ok(data) = field.bytes().await {
                        let _ = tokio::fs::write(&path, data).await;
                        #[cfg(target_os = "linux")]
                        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
                    }
                }
            }
        }
    }
    StatusCode::OK
}

pub async fn run_script_handler(Json(req): Json<RunScriptReq>) -> impl IntoResponse {
    let Some(path) = validate_script_path(SCRIPT_DIRECTORY, &req.name) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if !path.exists() { return StatusCode::NOT_FOUND.into_response(); }
    let mut command = path.to_string_lossy().to_string();
    if req.name.to_lowercase().ends_with(".py") { command = format!("python {}", command); }

    if req.script_type == "foreground" {
        match Command::new("sh").arg("-c").arg(command).output().await {
            Ok(output) => {
                let log = String::from_utf8_lossy(&output.stdout).to_string() + &String::from_utf8_lossy(&output.stderr);
                Json(RunScriptRsp { log }).into_response()
            }
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    } else {
        tokio::spawn(async move { let _ = Command::new("sh").arg("-c").arg(command).status().await; });
        Json(RunScriptRsp { log: "Started in background".to_string() }).into_response()
    }
}

pub async fn delete_script_handler(Json(req): Json<DeleteScriptReq>) -> impl IntoResponse {
    let Some(path) = validate_script_path(SCRIPT_DIRECTORY, &req.name) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match tokio::fs::remove_file(path).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn get_virtual_device_handler() -> impl IntoResponse {
    let network = std::path::Path::new(VIRTUAL_NETWORK).exists();
    let disk = std::path::Path::new(VIRTUAL_DISK).exists();
    Json(GetVirtualDeviceRsp { network, disk })
}

pub async fn update_virtual_device_handler(State(state): State<Arc<AppState>>, Json(req): Json<UpdateVirtualDeviceReq>) -> impl IntoResponse {
    let (device_path, mount_cmds, unmount_commands) = match req.device.as_str() {
        "network" => (VIRTUAL_NETWORK, vec!["touch /boot/usb.rndis0", "/etc/init.d/S03usbdev stop", "/etc/init.d/S03usbdev start"], vec!["/etc/init.d/S03usbdev stop", "rm -rf /sys/kernel/config/usb_gadget/g0/configs/c.1/rndis.usb0", "rm /boot/usb.rndis0", "/etc/init.d/S03usbdev start"]),
        "disk" => (VIRTUAL_DISK, vec!["touch /boot/usb.disk0", "/etc/init.d/S03usbdev stop", "/etc/init.d/S03usbdev start"], vec!["/etc/init.d/S03usbdev stop", "rm -rf /sys/kernel/config/usb_gadget/g0/configs/c.1/mass_storage.disk0", "rm /boot/usb.disk0", "/etc/init.d/S03usbdev start"]),
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    let exists = std::path::Path::new(device_path).exists();
    let cmds = if !exists { mount_cmds } else { unmount_commands };
    {
        let _hid = state.hid.lock().await;
        for cmd in cmds { let _ = Command::new("sh").arg("-c").arg(cmd).status().await; }
    }
    let on = std::path::Path::new(device_path).exists();
    Json(UpdateVirtualDeviceRsp { on }).into_response()
}

pub async fn get_mdns_handler() -> impl IntoResponse {
    let enabled = std::path::Path::new(AVAHI_PID_FILE).exists();
    Json(GetMdnsStateRsp { enabled })
}

pub async fn enable_mdns_handler() -> impl IntoResponse {
    let cmd = format!("cp -f {} {} && {} start", AVAHI_BACKUP_SCRIPT, AVAHI_SCRIPT, AVAHI_SCRIPT);
    match Command::new("sh").arg("-c").arg(cmd).status().await {
        Ok(s) if s.success() => StatusCode::OK,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn disable_mdns_handler() -> impl IntoResponse {
    if let Ok(pid) = tokio::fs::read_to_string(AVAHI_PID_FILE).await {
        let cmd = format!("kill -9 {} && rm -f {} {}", pid.trim(), AVAHI_PID_FILE, AVAHI_SCRIPT);
        match Command::new("sh").arg("-c").arg(cmd).status().await {
            Ok(s) if s.success() => StatusCode::OK,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    } else { StatusCode::OK }
}

pub async fn get_ssh_handler() -> impl IntoResponse {
    let enabled = !std::path::Path::new(SSH_STOP_FLAG).exists();
    Json(GetSSHStateRsp { enabled })
}

pub async fn enable_ssh_handler() -> impl IntoResponse {
    match Command::new("sh").arg("-c").arg(format!("{} permanent_on", SSH_SCRIPT)).status().await {
        Ok(s) if s.success() => StatusCode::OK,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn disable_ssh_handler() -> impl IntoResponse {
    match Command::new("sh").arg("-c").arg(format!("{} permanent_off", SSH_SCRIPT)).status().await {
        Ok(s) if s.success() => StatusCode::OK,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn get_swap_handler() -> impl IntoResponse {
    let size = std::fs::metadata(SWAP_FILE).map(|m| m.len() / 1024 / 1024).unwrap_or(0);
    Json(GetSwapRsp { size: size as i64 })
}

pub async fn set_swap_handler(Json(req): Json<SetSwapReq>) -> impl IntoResponse {
    let current_size = std::fs::metadata(SWAP_FILE).map(|m| m.len() / 1024 / 1024).unwrap_or(0);
    if req.size == current_size as i64 {
        return StatusCode::OK;
    }

    if req.size == 0 {
        let _ = run_shell_command("swapoff -a && rm -f /swapfile").await;
        let _ = disable_inittab_swap().await;
    } else {
        if current_size > 0 {
            let _ = run_shell_command("swapoff -a && rm -f /swapfile").await;
        }
        let cmd = format!("fallocate -l {}M {} && chmod 600 {} && mkswap {} && swapon {}", req.size, SWAP_FILE, SWAP_FILE, SWAP_FILE, SWAP_FILE);
        if run_shell_command(&cmd).await {
            let _ = enable_inittab_swap().await;
        }
    }
    StatusCode::OK
}

async fn enable_inittab_swap() -> tokio::io::Result<()> {
    let mut file = tokio::fs::OpenOptions::new().append(true).open(INITTAB_PATH).await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(format!("\nsi11::sysinit:/sbin/swapon {}", SWAP_FILE).as_bytes()).await?;
    Ok(())
}

async fn disable_inittab_swap() -> tokio::io::Result<()> {
    if let Ok(content) = tokio::fs::read_to_string(INITTAB_PATH).await {
        let new_content: Vec<&str> = content.lines()
            .filter(|line| !line.contains(SWAP_FILE))
            .collect();
        tokio::fs::write(INITTAB_PATH, new_content.join("\n")).await?;
    }
    Ok(())
}

pub async fn get_hostname_handler() -> impl IntoResponse {
    let hostname = tokio::fs::read_to_string(ETC_HOSTNAME).await.unwrap_or_default().trim().to_string();
    Json(GetHostnameRsp { hostname })
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
        return (StatusCode::BAD_REQUEST, "Invalid hostname").into_response();
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
    StatusCode::OK
}

pub async fn get_web_title_handler() -> impl IntoResponse {
    let title = tokio::fs::read_to_string(WEB_TITLE_FILE).await.unwrap_or_else(|_| "NanoKVM".to_string()).trim().to_string();
    Json(GetWebTitleRsp { title })
}

pub async fn set_web_title_handler(Json(req): Json<SetWebTitleReq>) -> impl IntoResponse {
    if req.title.is_empty() || req.title == "NanoKVM" { let _ = tokio::fs::remove_file(WEB_TITLE_FILE).await; }
    else { let _ = tokio::fs::write(WEB_TITLE_FILE, &req.title).await; }
    StatusCode::OK
}

pub async fn get_autostart_handler() -> impl IntoResponse {
    let mut files = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(AUTOSTART_DIRECTORY).await {
        while let Ok(Some(entry)) = entries.next_entry().await { files.push(entry.file_name().to_string_lossy().to_string()); }
    }
    Json(GetAutostartRsp { files })
}

pub async fn get_autostart_content_handler(Path(name): Path<String>) -> impl IntoResponse {
    let path = std::path::Path::new(AUTOSTART_DIRECTORY).join(name);
    match tokio::fs::read_to_string(path).await { Ok(c) => (StatusCode::OK, c).into_response(), Err(_) => StatusCode::NOT_FOUND.into_response(), }
}

pub async fn upload_autostart_handler(Path(name): Path<String>, Json(req): Json<UploadAutostartReq>) -> impl IntoResponse {
    let _ = tokio::fs::create_dir_all(AUTOSTART_DIRECTORY).await;
    let path = std::path::Path::new(AUTOSTART_DIRECTORY).join(name);
    match tokio::fs::write(&path, &req.content).await { Ok(_) => { #[cfg(target_os = "linux")] let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)); StatusCode::OK }, Err(_) => StatusCode::INTERNAL_SERVER_ERROR, }
}

pub async fn delete_autostart_handler(Path(name): Path<String>) -> impl IntoResponse {
    let path = std::path::Path::new(AUTOSTART_DIRECTORY).join(name);
    match tokio::fs::remove_file(path).await { Ok(_) => StatusCode::OK, Err(_) => StatusCode::NOT_FOUND, }
}

pub async fn reboot_handler() -> impl IntoResponse {
    if let Err(e) = Command::new("reboot").status().await {
        error!("Failed to execute reboot: {}", e);
    }
    StatusCode::OK
}



pub async fn reset_hdmi_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {

    let _ = state.kvm.set_hdmi(false);

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let _ = state.kvm.set_hdmi(true);

    let _ = tokio::fs::remove_file(HDMI_DISABLE_FILE).await;

    StatusCode::OK

}



pub async fn enable_hdmi_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {

    let _ = state.kvm.set_hdmi(true);

    let _ = tokio::fs::remove_file(HDMI_DISABLE_FILE).await;

    StatusCode::OK

}



pub async fn disable_hdmi_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {

    let _ = state.kvm.set_hdmi(false);

    let _ = tokio::fs::write(HDMI_DISABLE_FILE, b"").await;

    StatusCode::OK

}



pub async fn get_hdmi_state_handler() -> impl IntoResponse {

    let enabled = !std::path::Path::new(HDMI_DISABLE_FILE).exists();

    Json(GetHdmiStateRsp { enabled })

}
