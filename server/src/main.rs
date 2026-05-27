mod api;
mod application;
mod auth;
mod config;
mod download;
mod hid;
mod network;
mod passkey;
mod quality;
mod storage;
mod storage_health;
mod tailscale;
mod utils;
mod vm;
mod webrtc;

use crate::api::ApiResponse;
use crate::download::*;
use crate::webrtc::transport::{
    IceCandidate, PeerConnectionManager, WebRtcConfig, WhepEndpoint, WhepRequest,
};
use crate::webrtc::ws_signaling::h264_ws_handler;
use crate::webrtc::H264Frame;
use axum::http::{header, Method, StatusCode};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, State, WebSocketUpgrade,
    },
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::{broadcast, Mutex};
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
#[allow(unused_imports)]
use tracing::{debug, error, info, warn};

use crate::quality::SharedQualityController;
#[cfg(target_os = "linux")]
use crate::webrtc::screen::{stop_frame_detect_handler, update_frame_detect_handler};

#[cfg(target_os = "linux")]
use crate::vm::{
    delete_autostart_handler, delete_script_handler, disable_hdmi_handler, disable_mdns_handler,
    disable_ssh_handler, enable_hdmi_handler, enable_mdns_handler, enable_ssh_handler,
    get_autostart_content_handler, get_autostart_handler, get_gpio_handler, get_hardware_handler,
    get_hdmi_state_handler, get_hostname_handler, get_info_handler, get_jiggler_handler,
    get_mdns_handler, get_memory_limit_handler, get_oled_handler, get_scripts_handler,
    get_ssh_handler, get_swap_handler, get_virtual_device_handler, get_web_title_handler,
    reboot_handler, reset_hdmi_handler, run_script_handler, set_gpio_handler, set_hostname_handler,
    set_jiggler_handler, set_memory_limit_handler, set_oled_handler, set_screen_handler,
    set_swap_handler, set_tls_handler, set_web_title_handler, terminal_handler,
    update_virtual_device_handler, upload_autostart_handler, upload_script_handler,
};

use crate::application::{
    get_preview_handler, get_version_handler, offline_update_handler, set_preview_handler,
    update_handler,
};
use crate::auth::brute_force::BruteForce;
use crate::auth::{
    auth_middleware, change_password_handler, get_account_handler, get_encryption_key_handler,
    is_password_updated_handler, login_handler, logout_handler,
};
use crate::config::Config;
use crate::hid::{
    add_shortcut_handler, delete_shortcut_handler, get_hid_mode_handler, get_leader_key_handler,
    get_shortcuts_handler, paste_handler, reset_hid_handler,
    set_hid_mode_handler, set_leader_key_handler,
};
use crate::network::{
    connect_wifi_handler, connect_wifi_no_auth_handler, delete_wol_mac_handler,
    disconnect_wifi_handler, get_wifi_handler, get_wol_macs_handler, set_wol_name_handler,
    wol_handler,
};

#[cfg(target_os = "linux")]
use crate::network::{get_ethernet_config_handler, set_ethernet_config_handler};
use crate::passkey::handlers::{
    enroll_complete_handler, login_challenge_handler, login_verify_handler, passkey_setup_handler,
    passkey_status_handler, qr_code_handler, recover_handler, recovery_download_handler,
};
use crate::storage::{
    delete_image_handler, get_cdrom_handler, get_images_handler, get_mounted_image_handler,
    mount_image_handler,
};
use crate::storage_health::{
    detailed_health_handler, health_handler, health_status_handler, refresh_health_handler,
    HealthState,
};
#[cfg(target_os = "linux")]
use crate::tailscale::{
    get_auto_update_handler, get_version_handler as get_tailscale_version_handler,
    init_auto_update, set_auto_update_handler, spawn_auto_update_task, tailscale_down_handler,
    tailscale_install_handler, tailscale_login_handler, tailscale_logout_handler,
    tailscale_restart_handler, tailscale_start_handler, tailscale_status_handler,
    tailscale_stop_handler, tailscale_uninstall_handler, tailscale_up_handler,
    trigger_update_handler,
};
#[cfg(not(target_os = "linux"))]
use crate::vm::{
    delete_script_handler, get_info_handler, get_memory_limit_handler, get_oled_handler,
    get_scripts_handler, run_script_handler, set_memory_limit_handler, set_oled_handler,
    terminal_handler, upload_script_handler,
};
use axum_server::tls_rustls::RustlsConfig;
use std::net::SocketAddr;

/// Parse a socket address from port, exiting on failure.
fn bind_addr(port: u16) -> SocketAddr {
    format!("0.0.0.0:{}", port).parse().unwrap_or_else(|e| {
        error!("Invalid port {}: {}", port, e);
        std::process::exit(1);
    })
}

/// Create a TCP listener with optimized settings for low latency
/// - TCP_NODELAY: Disable Nagle's algorithm for immediate packet sending
/// - SO_REUSEADDR: Allow quick rebinding after restart
async fn create_optimized_listener(addr: SocketAddr) -> std::io::Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};

    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.set_nodelay(true)?; // TCP_NODELAY on listener (inherited by some systems)
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;

    TcpListener::from_std(socket.into())
}

// Shared application state
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub struct AppState {
    config: Arc<Config>,
    screen_config: crate::webrtc::screen::SharedScreenConfig,
    quality_controller: SharedQualityController,
    tx_mjpeg: broadcast::Sender<Bytes>,
    tx_h264: broadcast::Sender<H264Frame>,
    tx_audio: broadcast::Sender<Bytes>,
    hid: Arc<Mutex<::hid::HidEngine>>,
    webrtc: Arc<PeerConnectionManager>,
    whep: Arc<WhepEndpoint>,
    vm: Arc<::vm::gpio::VmController>,
    jiggler: Arc<::vm::jiggler::MouseJiggler>,
    kvm: Arc<::kvm::Kvm>,
    health_state: Arc<HealthState>,
    passkey_state: Arc<crate::passkey::PasskeyState>,
    brute_force: Arc<BruteForce>,
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)] // Fields for future audio streaming support
pub struct AppState {
    config: Arc<Config>,
    screen_config: crate::webrtc::screen::SharedScreenConfig,
    quality_controller: SharedQualityController,
    tx_mjpeg: broadcast::Sender<Bytes>,
    tx_h264: broadcast::Sender<H264Frame>,
    tx_audio: broadcast::Sender<Bytes>,
    hid: Arc<Mutex<::hid::HidEngine>>,
    webrtc: Arc<PeerConnectionManager>,
    whep: Arc<WhepEndpoint>,
    jiggler: Arc<::vm::jiggler::MouseJiggler>,
    health_state: Arc<HealthState>,
    passkey_state: Arc<crate::passkey::PasskeyState>,
    brute_force: Arc<BruteForce>,
}

#[tokio::main]
async fn main() {
    // Install rustls crypto provider (required for TLS support)
    // Uses ring as the crypto backend which works on all platforms including RISC-V
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Create shutdown broadcast channel
    let (_shutdown_tx, _) = broadcast::channel::<()>(1);

    // 1. Load Configuration
    let config = Arc::new(Config::load().await);

    // 2. Initialize Logging
    let _guard = init_logging(&config);
    info!("NanoKVM Rust Server Starting...");

    // 3. Initialize Tailscale auto-updater
    #[cfg(target_os = "linux")]
    {
        init_auto_update().await;
        spawn_auto_update_task();
    }

    let screen_config = Arc::new(parking_lot::RwLock::new(
        crate::webrtc::screen::ScreenConfig::new(),
    ));

    // 4. Initialize Hardware & Loops
    #[cfg(target_os = "linux")]
    let (kvm_handle, hid_engine, vm_controller, mouse_jiggler) = {
        let kvm = Arc::new(::kvm::Kvm::init());
        let hid = Arc::new(Mutex::new(::hid::HidEngine::new().await));
        let vm = Arc::new(::vm::gpio::VmController::new().await);
        let jiggler = Arc::new(::vm::jiggler::MouseJiggler::new(hid.clone()));
        jiggler.spawn_loop().await;
        (kvm, hid, vm, jiggler)
    };

    #[cfg(not(target_os = "linux"))]
    let (hid_engine, mouse_jiggler) = {
        let hid = Arc::new(Mutex::new(::hid::HidEngine::new().await));
        let jiggler = Arc::new(::vm::jiggler::MouseJiggler::new(hid.clone()));
        jiggler.spawn_loop().await;
        (hid, jiggler)
    };

    // Create Broadcast Channels - reduced buffer size for lower latency
    // Smaller buffers (4 vs 16) reduce frame queuing delay at cost of potential drops under load
    let (tx_mjpeg, _rx) = broadcast::channel::<Bytes>(4);
    let (tx_h264, _rx) = broadcast::channel::<H264Frame>(4);
    let (tx_audio, _rx) = broadcast::channel::<Bytes>(4);

    // Initialize WebRTC
    let mut webrtc_config_builder = WebRtcConfig::builder();
    if !config.stun.is_empty() {
        webrtc_config_builder =
            webrtc_config_builder.add_stun_server(format!("stun:{}", config.stun));
    }
    if !config.turn.turn_addr.is_empty() {
        webrtc_config_builder = webrtc_config_builder.add_turn_server(
            format!("turn:{}", config.turn.turn_addr),
            config.turn.turn_user.clone(),
            config.turn.turn_cred.clone(),
        );
    }
    let webrtc_config = webrtc_config_builder
        .build()
        .expect("invalid WebRTC config");

    let webrtc_manager = Arc::new(
        PeerConnectionManager::new(webrtc_config)
            .await
            .expect("WebRTC init failed"),
    );
    let whep_endpoint = Arc::new(WhepEndpoint::new(webrtc_manager.clone()));

    let passkey_state = Arc::new(crate::passkey::PasskeyState::new());
    let quality_controller = crate::quality::new_shared();

    #[cfg(target_os = "linux")]
    let shared_state = Arc::new(AppState {
        config: config.clone(),
        screen_config: screen_config.clone(),
        quality_controller: quality_controller.clone(),
        tx_mjpeg,
        tx_h264: tx_h264.clone(),
        tx_audio: tx_audio.clone(),
        hid: hid_engine.clone(),
        webrtc: webrtc_manager.clone(),
        whep: whep_endpoint.clone(),
        vm: vm_controller.clone(),
        jiggler: mouse_jiggler.clone(),
        kvm: kvm_handle.clone(),
        health_state: Arc::new(HealthState::default()),
        passkey_state: passkey_state.clone(),
        brute_force: {
            let bf = Arc::new(BruteForce::new(config.security.clone()));
            bf.spawn_cleanup();
            bf
        },
    });

    #[cfg(not(target_os = "linux"))]
    let shared_state = Arc::new(AppState {
        config: config.clone(),
        screen_config: screen_config.clone(),
        quality_controller: quality_controller.clone(),
        tx_mjpeg,
        tx_h264: tx_h264.clone(),
        tx_audio: tx_audio.clone(),
        hid: hid_engine.clone(),
        webrtc: webrtc_manager.clone(),
        whep: whep_endpoint.clone(),
        jiggler: mouse_jiggler.clone(),
        health_state: Arc::new(HealthState::default()),
        passkey_state,
        brute_force: {
            let bf = Arc::new(BruteForce::new(config.security.clone()));
            bf.spawn_cleanup();
            bf
        },
    });

    // Initialize storage health check on boot (non-blocking)
    {
        let health_state = shared_state.health_state.clone();
        tokio::spawn(async move {
            let health = ::storage::health::check_health_with_logging().await;
            let mut cached = health_state.cached_health.lock().await;
            *cached = Some(health);
        });
    }

    // Spawn Producer Tasks (Linux Only)
    #[cfg(target_os = "linux")]
    {
        let s1 = shared_state.clone();
        tokio::spawn(async move {
            mjpeg_hardware_loop(s1).await;
        });
        let s2 = shared_state.clone();
        tokio::spawn(async move {
            h264_hardware_loop(s2).await;
        });
        #[cfg(feature = "audio")]
        {
            let s3 = shared_state.clone();
            tokio::spawn(async move {
                audio_hardware_loop(s3).await;
            });
        }

        // Periodic storage health check every 24 hours
        let health_state = shared_state.health_state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_hours(24)).await;
                let health = ::storage::health::check_health_with_logging().await;
                let mut cached = health_state.cached_health.lock().await;
                *cached = Some(health);
            }
        });
    }

    // Use absolute path to web assets - /kvmapp/server/web has the actual content
    // /tmp/server/web may have empty files due to init script copy issues
    let web_path = "/kvmapp/server/web";

    // 4. Build Router
    #[allow(unused_mut)] // mut needed for cfg(target_os = "linux") blocks
    let mut api_routes = Router::new()
        .route("/application/version", get(get_version_handler))
        .route("/application/update", post(update_handler))
        .route("/application/update/offline", post(offline_update_handler))
        .route(
            "/application/preview",
            get(get_preview_handler).post(set_preview_handler),
        )
        .route("/download/image/enabled", get(image_enabled_handler))
        .route("/download/image/status", get(status_image_handler))
        .route("/download/image/file", post(upload_image_handler))
        // Go frontend uses this legacy upload endpoint.
        .route("/download/file", post(upload_image_handler))
        .route("/download/image", post(download_image_url_handler))
        .route("/stream/mjpeg", get(mjpeg_stream))
        .route("/stream/h264", get(h264_ws_handler))
        .route("/stream/h264/direct", get(h264_direct_handler))
        .route("/vm/info", get(get_info_handler))
        .route("/vm/oled", get(get_oled_handler).post(set_oled_handler))
        .route("/vm/terminal", get(terminal_handler))
        .route(
            "/vm/script",
            get(get_scripts_handler)
                .post(upload_script_handler)
                .delete(delete_script_handler),
        )
        // Go frontend expects a dedicated upload endpoint.
        .route("/vm/script/upload", post(upload_script_handler))
        .route("/vm/script/run", post(run_script_handler))
        .route(
            "/vm/memory/limit",
            get(get_memory_limit_handler).post(set_memory_limit_handler),
        )
        .route("/storage/image", get(get_images_handler))
        .route("/storage/image/mounted", get(get_mounted_image_handler))
        .route("/storage/image/mount", post(mount_image_handler))
        .route("/storage/image/delete", post(delete_image_handler))
        .route(
            "/storage/images",
            get(get_images_handler).delete(delete_image_handler),
        )
        .route("/storage/mount", post(mount_image_handler))
        .route("/storage/mounted", get(get_mounted_image_handler))
        .route("/storage/cdrom", get(get_cdrom_handler))
        .route("/storage/health/status", get(health_status_handler))
        .route(
            "/storage/health",
            get(health_handler).post(refresh_health_handler),
        )
        .route("/storage/health/detailed", get(detailed_health_handler))
        .route("/hid/paste", post(paste_handler))
        .route("/hid/shortcuts", get(get_shortcuts_handler))
        .route(
            "/hid/shortcut",
            post(add_shortcut_handler).delete(delete_shortcut_handler),
        )
        .route(
            "/hid/shortcut/leader-key",
            get(get_leader_key_handler).post(set_leader_key_handler),
        )
        .route(
            "/hid/mode",
            get(get_hid_mode_handler).post(set_hid_mode_handler),
        )
        .route("/hid/reset", post(reset_hid_handler))
        .route(
            "/network/wol",
            post(wol_handler)
                .get(get_wol_macs_handler)
                .delete(delete_wol_mac_handler),
        )
        .route("/network/wol/name", post(set_wol_name_handler))
        // Go frontend routes for WoL MAC history management.
        .route(
            "/network/wol/mac",
            get(get_wol_macs_handler).delete(delete_wol_mac_handler),
        )
        .route("/network/wol/mac/name", post(set_wol_name_handler))
        // Go frontend routes for Wi-Fi.
        .route("/network/wifi", get(get_wifi_handler))
        .route("/network/wifi/connect", post(connect_wifi_handler))
        .route("/network/wifi/disconnect", post(disconnect_wifi_handler))
        .route("/ws", get(ws_handler));

    #[cfg(target_os = "linux")]
    {
        api_routes = api_routes.route(
            "/network/ethernet",
            get(get_ethernet_config_handler).post(set_ethernet_config_handler),
        );
    }

    #[cfg(target_os = "linux")]
    {
        api_routes = api_routes
            .route("/stream/mjpeg/detect", post(update_frame_detect_handler))
            .route("/stream/mjpeg/detect/stop", post(stop_frame_detect_handler));
    }

    // Quality control API (works on all platforms)
    api_routes = api_routes.route(
        "/stream/quality",
        get(get_quality_handler).post(set_quality_auto_handler),
    );

    #[cfg(target_os = "linux")]
    {
        api_routes = api_routes
            .route("/vm/hardware", get(get_hardware_handler))
            .route(
                "/vm/device/virtual",
                get(get_virtual_device_handler).post(update_virtual_device_handler),
            )
            .route("/vm/gpio", get(get_gpio_handler).post(set_gpio_handler))
            .route(
                "/vm/mouse-jiggler",
                get(get_jiggler_handler).post(set_jiggler_handler),
            )
            // Go backend historically accepted a trailing slash here.
            .route(
                "/vm/mouse-jiggler/",
                get(get_jiggler_handler).post(set_jiggler_handler),
            )
            .route("/vm/mdns", get(get_mdns_handler))
            .route("/vm/mdns/enable", post(enable_mdns_handler))
            .route("/vm/mdns/disable", post(disable_mdns_handler))
            .route("/vm/ssh", get(get_ssh_handler))
            .route("/vm/ssh/enable", post(enable_ssh_handler))
            .route("/vm/ssh/disable", post(disable_ssh_handler))
            .route("/vm/swap", get(get_swap_handler).post(set_swap_handler))
            .route(
                "/vm/hostname",
                get(get_hostname_handler).post(set_hostname_handler),
            )
            .route(
                "/vm/web-title",
                get(get_web_title_handler).post(set_web_title_handler),
            )
            .route("/vm/autostart", get(get_autostart_handler))
            .route(
                "/vm/autostart/{name}",
                get(get_autostart_content_handler)
                    .post(upload_autostart_handler)
                    .delete(delete_autostart_handler),
            )
            .route("/vm/tls", post(set_tls_handler))
            .route("/vm/hdmi", get(get_hdmi_state_handler))
            .route("/vm/hdmi/reset", post(reset_hdmi_handler))
            .route("/vm/hdmi/enable", post(enable_hdmi_handler))
            .route("/vm/hdmi/disable", post(disable_hdmi_handler))
            .route("/vm/screen", post(set_screen_handler))
            .route("/vm/reboot", post(reboot_handler))
            // Go frontend expects this path.
            .route("/vm/system/reboot", post(reboot_handler))
            .route("/tailscale/install", post(tailscale_install_handler))
            .route("/tailscale/uninstall", post(tailscale_uninstall_handler))
            .route("/tailscale/start", post(tailscale_start_handler))
            .route("/tailscale/restart", post(tailscale_restart_handler))
            .route("/tailscale/stop", post(tailscale_stop_handler))
            .route("/tailscale/status", get(tailscale_status_handler))
            .route("/tailscale/login", post(tailscale_login_handler))
            .route("/tailscale/up", post(tailscale_up_handler))
            .route("/tailscale/down", post(tailscale_down_handler))
            .route("/tailscale/logout", post(tailscale_logout_handler))
            .route(
                "/tailscale/auto-update",
                get(get_auto_update_handler).post(set_auto_update_handler),
            )
            // Go frontend (extensions) alias routes.
            .route(
                "/extensions/tailscale/install",
                post(tailscale_install_handler),
            )
            .route(
                "/extensions/tailscale/uninstall",
                post(tailscale_uninstall_handler),
            )
            .route("/extensions/tailscale/start", post(tailscale_start_handler))
            .route(
                "/extensions/tailscale/restart",
                post(tailscale_restart_handler),
            )
            .route("/extensions/tailscale/stop", post(tailscale_stop_handler))
            .route(
                "/extensions/tailscale/status",
                get(tailscale_status_handler),
            )
            .route("/extensions/tailscale/login", post(tailscale_login_handler))
            .route("/extensions/tailscale/up", post(tailscale_up_handler))
            .route("/extensions/tailscale/down", post(tailscale_down_handler))
            .route(
                "/extensions/tailscale/logout",
                post(tailscale_logout_handler),
            )
            .route(
                "/extensions/tailscale/auto-update",
                get(get_auto_update_handler).post(set_auto_update_handler),
            )
            .route("/tailscale/version", get(get_tailscale_version_handler))
            .route("/tailscale/update", post(trigger_update_handler));
    }

    let api_routes = api_routes
        .route("/webrtc/whep", post(whep_post_handler))
        .route(
            "/webrtc/whep/{id}",
            get(whep_get_handler)
                .patch(whep_patch_handler)
                .delete(whep_delete_handler),
        )
        .layer(middleware::from_fn_with_state(
            shared_state.clone(),
            auth_middleware,
        ));

    let app = Router::new()
        .route("/health", get(health_check_handler))
        .route("/api/system/capabilities", get(capabilities_handler))
        // Auth routes (outside of api_routes so they don't require auth)
        .route("/api/login", post(login_handler))
        .route("/api/logout", post(logout_handler))
        .route("/api/auth/login", post(login_handler))
        .route("/api/auth/account", get(get_account_handler))
        .route("/api/auth/encryption-key", get(get_encryption_key_handler))
        .route(
            "/api/auth/password",
            get(is_password_updated_handler).post(change_password_handler),
        )
        .route("/api/auth/logout", post(logout_handler))
        // Passkey authentication routes (unauthenticated)
        .route("/api/passkey/status", get(passkey_status_handler))
        .route("/api/passkey/setup", post(passkey_setup_handler))
        .route("/api/passkey/enroll", post(enroll_complete_handler))
        .route(
            "/api/passkey/login/challenge",
            post(login_challenge_handler),
        )
        .route("/api/passkey/login/verify", post(login_verify_handler))
        .route("/api/passkey/recover", post(recover_handler))
        .route(
            "/api/passkey/recovery/download",
            get(recovery_download_handler),
        )
        .route("/api/passkey/qr", get(qr_code_handler))
        // Connect Wi-Fi without auth (only in AP mode), matching Go backend.
        .route("/api/network/wifi", post(connect_wifi_no_auth_handler))
        .nest("/api", api_routes)
        .fallback_service(ServeDir::new(web_path))
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::predicate(|origin, _req_head| {
                    // Allow same-origin requests and localhost (development)
                    if let Ok(origin_str) = origin.to_str() {
                        origin_str.starts_with("http://localhost")
                            || origin_str.starts_with("http://127.0.0.1")
                            || origin_str.starts_with("https://localhost")
                            || origin_str.starts_with("https://127.0.0.1")
                    } else {
                        false
                    }
                }))
                .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
                .max_age(Duration::from_secs(3600)),
        )
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            header::HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            header::HeaderValue::from_static("nosniff"),
        ))
        .with_state(shared_state);

    // 5. Start Server
    if config.proto == "https" {
        let https_addr = bind_addr(config.port.https);
        let http_addr = bind_addr(config.port.http);

        let tls_config = match RustlsConfig::from_pem_file(&config.cert.crt, &config.cert.key).await
        {
            Ok(cfg) => cfg,
            Err(e) => {
                error!(
                    "Failed to load TLS certs from {} and {}: {}",
                    config.cert.crt, config.cert.key, e
                );
                error!("Falling back to HTTP mode. To use HTTPS, ensure certificate files exist.");
                // Fall through to HTTP mode
                let http_addr = bind_addr(config.port.http);
                let listener = create_optimized_listener(http_addr)
                    .await
                    .expect("HTTP bind failed");
                info!(
                    "Server listening on {} (HTTP - TLS fallback, TCP_NODELAY enabled)",
                    http_addr
                );
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown_signal())
                    .await
                    .expect("HTTP server failed");
                return;
            }
        };

        let redirect_app = Router::new().fallback(
            move |host: axum::http::HeaderMap, uri: axum::http::Uri| async move {
                let host = host
                    .get(header::HOST)
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("localhost");
                let https_uri = if config.port.https == 443 {
                    format!("https://{}{}", host, uri)
                } else {
                    format!("https://{}:{}{}", host, config.port.https, uri)
                };
                (
                    StatusCode::TEMPORARY_REDIRECT,
                    [(header::LOCATION, https_uri)],
                )
                    .into_response()
            },
        );

        info!("Starting HTTP redirector on {}", http_addr);
        tokio::spawn(async move {
            let listener = create_optimized_listener(http_addr)
                .await
                .expect("HTTP bind failed");
            axum::serve(listener, redirect_app)
                .await
                .expect("HTTP redirect failed");
        });

        info!("Server listening on {} (HTTPS)", https_addr);
        let handle = axum_server::Handle::new();
        let shutdown_handle = handle.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            shutdown_handle.graceful_shutdown(Some(Duration::from_secs(10)));
        });
        axum_server::bind_rustls(https_addr, tls_config)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .expect("HTTPS server failed");
    } else {
        let http_addr = bind_addr(config.port.http);
        let listener = create_optimized_listener(http_addr)
            .await
            .expect("HTTP bind failed");
        info!(
            "Server listening on {} (HTTP, TCP_NODELAY enabled)",
            http_addr
        );
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .expect("HTTP server failed");
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received, starting graceful shutdown...");

    // Cleanup KVM hardware resources
    #[cfg(target_os = "linux")]
    {
        ::kvm::Kvm::deinit();
    }
}

fn init_logging(config: &Config) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.logger.level));
    if config.logger.file == "stdout" {
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(env_filter)
            .init();
        None
    } else {
        let path = std::path::Path::new(&config.logger.file);
        let directory = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let file_name = path
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("nanokvm.log"));
        let file_appender = tracing_appender::rolling::daily(directory, file_name);
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::registry()
            .with(fmt::layer().with_writer(non_blocking))
            .with(env_filter)
            .init();
        Some(guard)
    }
}

#[allow(dead_code)] // TODO: Call from video capture task for improved latency
fn set_realtime_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let mut param: libc::sched_param = std::mem::zeroed();
        param.sched_priority = 10;
        if libc::sched_setscheduler(0, libc::SCHED_FIFO, &param) != 0 {
            warn!("Failed to set real-time priority");
        }
    }
}

#[cfg(target_os = "linux")]
async fn mjpeg_hardware_loop(state: Arc<AppState>) {
    set_realtime_priority();
    let mut interval = tokio::time::interval(Duration::from_millis(33));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let (width, height, manual_quality, fps, stream_type) = {
            let cfg = state.screen_config.read();
            (
                cfg.width,
                cfg.height,
                cfg.quality,
                cfg.fps,
                cfg.stream_type.clone(),
            )
        };
        // Use adaptive quality if enabled, otherwise manual
        let quality = state.quality_controller.get_mjpeg_quality(manual_quality);

        interval.tick().await;
        // Only read MJPEG when the configured stream type is MJPEG.
        // Rationale: stale WebRTC/direct-H.264 sessions can otherwise permanently starve MJPEG
        // (HTTP handler stays connected, but never receives any frames).
        if state.tx_mjpeg.receiver_count() > 0 && stream_type == "mjpeg" {
            let kvm_state = state.clone();
            match tokio::task::spawn_blocking(move || {
                kvm_state.kvm.get_mjpeg(width, height, quality)
            })
            .await
            {
                Ok(Ok(frame)) => {
                    // Track send success for adaptive quality
                    let send_result = state.tx_mjpeg.send(frame.into_bytes());
                    let success = send_result.is_ok();
                    state.quality_controller.on_frame_result(success);
                }
                Ok(Err(e)) => {
                    // Log KVM errors (but not too frequently for expected conditions)
                    match &e {
                        kvm::KvmError::NotExist => {}         // Frame not ready, normal
                        kvm::KvmError::Retrieving => {}       // Still retrieving, normal
                        kvm::KvmError::NotInitialized => {}   // No HDMI, will retry
                        kvm::KvmError::LibraryNotLoaded => {} // No libkvm.so
                        _ => tracing::warn!("MJPEG capture error: {}", e),
                    }
                }
                Err(e) => {
                    tracing::error!("MJPEG spawn_blocking error: {}", e);
                }
            }
        }
        let frame_interval = Duration::from_secs_f64(1.0 / fps as f64);
        if interval.period() != frame_interval {
            interval = tokio::time::interval(frame_interval);
        }
    }
}

#[cfg(target_os = "linux")]
async fn h264_hardware_loop(state: Arc<AppState>) {
    set_realtime_priority();
    {
        let cfg = state.screen_config.read();
        state.kvm.set_h264_gop(cfg.gop);
    }
    state.kvm.set_frame_detect(1);
    let mut interval = tokio::time::interval(Duration::from_millis(33));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pts = 0u64;
    let start_time = std::time::Instant::now();
    let mut frame_counter: u64 = 0;
    let mut parameter_sets_initialized = false;

    loop {
        let (width, height, manual_bitrate, fps, stream_type) = {
            let cfg = state.screen_config.read();
            (
                cfg.width,
                cfg.height,
                cfg.bitrate,
                cfg.fps,
                cfg.stream_type.clone(),
            )
        };
        // Use adaptive bitrate if enabled, otherwise manual
        let bitrate = state.quality_controller.get_h264_bitrate(manual_bitrate);

        interval.tick().await;
        // Only capture H.264 when the configured stream type is H.264.
        if stream_type != "h264" {
            continue;
        }
        // Read H.264 when either WebRTC peers are connected OR direct H.264 clients are subscribed.
        // Direct streaming uses `tx_h264` (no WebRTC connections), so we must not gate on WebRTC alone.
        let webrtc_active = state.webrtc.total_connection_count() > 0;
        let direct_receivers = state.tx_h264.receiver_count();
        if webrtc_active || direct_receivers > 0 {
            let kvm_state = state.clone();
            if let Ok(Ok(frame_result)) =
                tokio::task::spawn_blocking(move || kvm_state.kvm.get_h264(width, height, bitrate))
                    .await
            {
                // Use the frame_type returned by hardware to detect keyframes
                let is_keyframe = frame_result.frame_type == kvm::H264FrameType::IFrame;
                let frame_data = frame_result.into_bytes();

                // Extract and store SPS/PPS from keyframes for SDP negotiation
                if is_keyframe {
                    debug!("I-frame detected, {} bytes", frame_data.len());
                    let (sps, pps) =
                        crate::webrtc::transport::extract_h264_parameter_sets(&frame_data);
                    if let (Some(sps_data), Some(pps_data)) = (sps, pps) {
                        if !parameter_sets_initialized {
                            info!(
                                "H.264 parameter sets extracted: SPS {} bytes, PPS {} bytes",
                                sps_data.len(),
                                pps_data.len()
                            );
                        }
                        state
                            .webrtc
                            .update_h264_parameter_sets("default", sps_data, pps_data);
                        parameter_sets_initialized = true;
                    } else if !parameter_sets_initialized {
                        debug!(
                            "Failed to extract SPS/PPS from first I-frame. First 20 bytes: {:02x?}",
                            &frame_data[..std::cmp::min(20, frame_data.len())]
                        );
                    }
                }

                let packets = Arc::new(crate::webrtc::transport::packetize_h264_optimized(
                    &frame_data,
                ));
                let conn_ids = state.webrtc.get_source_connections("default");

                // Single success metric per frame for adaptive quality control:
                // - If only WebRTC is active, base on whether all peers received the frame.
                // - If only direct is active, base on whether broadcast channel delivered.
                // - If both are active, require both to succeed.
                let mut success_for_adaptation = true;

                if !conn_ids.is_empty() {
                    let sent = state
                        .webrtc
                        .broadcast_frame(conn_ids.clone(), pts as u32, &packets)
                        .await
                        .unwrap_or(0);
                    // Track WebRTC broadcast success (all connections received)
                    let all_sent = sent == conn_ids.len();
                    success_for_adaptation &= all_sent;
                    if fps > 0 && frame_counter % fps as u64 == 0 {
                        debug!(
                            "H.264 broadcast: {} connections, {} packets, sent to {} peers",
                            conn_ids.len(),
                            packets.len(),
                            sent
                        );
                    }
                }

                if direct_receivers > 0 {
                    let timestamp = start_time.elapsed().as_micros() as u64;
                    let h264_frame = H264Frame {
                        is_keyframe,
                        timestamp,
                        packets: packets.clone(),
                        raw_data: frame_data,
                    };
                    let direct_ok = state.tx_h264.send(h264_frame).is_ok();
                    success_for_adaptation &= direct_ok;
                }

                state
                    .quality_controller
                    .on_frame_result(success_for_adaptation);

                let fps_u64 = (fps.max(1)) as u64;
                let pts_step = (90_000u64 / fps_u64).max(1);
                pts = pts.wrapping_add(pts_step);
                frame_counter = frame_counter.wrapping_add(1);
            }
        }
        let frame_interval = Duration::from_secs_f64(1.0 / fps as f64);
        if interval.period() != frame_interval {
            interval = tokio::time::interval(frame_interval);
        }
    }
}

async fn mjpeg_stream(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    use bytes::BufMut;

    let rx = state.tx_mjpeg.subscribe();
    let s = BroadcastStream::new(rx)
        .filter_map(|r| async move { r.ok() })
        .map(|f| {
            // Optimized frame construction using BytesMut to minimize allocations
            // Header is ~60 bytes, frame is ~160KB, so pre-allocate slightly over
            let header_estimate = 70;
            let mut buf = bytes::BytesMut::with_capacity(header_estimate + f.len() + 2);

            // Write header directly without format! allocation
            buf.put_slice(b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: ");
            // Write length as ASCII digits using itoa::Buffer (fast integer formatting)
            let mut itoa_buf = itoa::Buffer::new();
            let len_str = itoa_buf.format(f.len());
            buf.put_slice(len_str.as_bytes());
            buf.put_slice(b"\r\n\r\n");
            buf.put_slice(&f);
            buf.put_slice(b"\r\n");

            Ok::<Bytes, axum::Error>(buf.freeze())
        });
    (
        [
            (
                header::CONTENT_TYPE,
                "multipart/x-mixed-replace; boundary=frame",
            ),
            (header::CACHE_CONTROL, "no-cache"),
            (header::CONNECTION, "keep-alive"),
            (header::PRAGMA, "no-cache"),
        ],
        axum::body::Body::from_stream(s),
    )
}

async fn h264_direct_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_h264_direct(socket, state))
}

async fn handle_h264_direct(mut socket: WebSocket, state: Arc<AppState>) {
    use bytes::BufMut;

    let mut rx = state.tx_h264.subscribe();
    while let Ok(f) = rx.recv().await {
        // Optimized: use BytesMut with exact capacity
        let mut buf = bytes::BytesMut::with_capacity(9 + f.raw_data.len());
        buf.put_u8(if f.is_keyframe { 1 } else { 0 });
        buf.put_u64_le(f.timestamp);
        buf.put_slice(&f.raw_data);
        if socket
            .send(Message::Binary(buf.freeze().into()))
            .await
            .is_err()
        {
            break;
        }
    }
}

async fn whep_post_handler(State(state): State<Arc<AppState>>, body: String) -> Response {
    let r = WhepRequest {
        source_id: "default".to_string(),
        offer_sdp: body,
    };
    match state.whep.create_resource(r).await {
        Ok(res) => Response::builder()
            .status(StatusCode::CREATED)
            .header(header::LOCATION, res.location)
            .header(header::ETAG, res.etag)
            .header(header::CONTENT_TYPE, "application/sdp")
            .body(axum::body::Body::from(res.answer_sdp))
            .unwrap(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn whep_get_handler(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if state.whep.get_resource(&id).is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn whep_patch_handler(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(c): Json<IceCandidate>,
) -> impl IntoResponse {
    match state.whep.add_ice_candidate(&id, c).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::NOT_FOUND,
    }
}

async fn whep_delete_handler(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.whep.delete_resource(&id).await {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::NOT_FOUND,
    }
}

#[cfg(all(target_os = "linux", feature = "audio"))]
async fn audio_hardware_loop(state: Arc<AppState>) {
    use audio::AudioCapturer;
    use opus::{Application, Channels, Encoder};

    let capturer = match AudioCapturer::new(0, 0) {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to initialize audio capturer: {}", e);
            return;
        }
    };

    let mut encoder = match Encoder::new(48000, Channels::Stereo, Application::Audio) {
        Ok(e) => e,
        Err(e) => {
            error!("Failed to initialize Opus encoder: {}", e);
            return;
        }
    };

    let mut pcm_buf = vec![0u8; 1024 * 2 * 2]; // 1024 samples, 2 channels, 16-bit
    let mut opus_buf = vec![0u8; 1024];
    let mut pts = 0u32;

    loop {
        // We don't need a ticker here because pcm_read blocks until data is available (period_size)
        match capturer.read(&mut pcm_buf) {
            Ok(_) => {
                // Convert bytes to i16 for Opus encoder
                let pcm_i16: Vec<i16> = pcm_buf
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]))
                    .collect();

                match encoder.encode(&pcm_i16, &mut opus_buf) {
                    Ok(size) => {
                        let payload = Bytes::copy_from_slice(&opus_buf[..size]);
                        let _ = state.tx_audio.send(payload.clone());

                        if state.webrtc.total_connection_count() > 0 {
                            let cids = state.webrtc.get_source_connections("default");
                            let _ = state.webrtc.broadcast_audio(cids, pts, &payload).await;
                        }
                        pts = pts.wrapping_add(960); // 20ms @ 48kHz
                    }
                    Err(e) => error!("Opus encoding failed: {}", e),
                }
            }
            Err(e) => {
                error!("Audio read error: {}", e);
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn health_check_handler() -> impl IntoResponse {
    Json(ApiResponse::ok(serde_json::json!({
        "service": "nanokvm",
        "status": "ok"
    })))
}

async fn capabilities_handler(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    // Get actual Tailscale capabilities
    let ts_caps = crate::tailscale::get_capabilities().await;
    let passkey_configured = std::path::Path::new("/etc/kvm/passkey.json").exists();

    Json(serde_json::json!({
        "tailscale_installed": ts_caps.installed,
        "tailscale_connected": ts_caps.connected,
        "tailscale_funnel_active": ts_caps.funnel_active,
        "passkey_configured": passkey_configured,
        "funnel_url": ts_caps.funnel_url
    }))
}

/// Get current quality stats
async fn get_quality_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(ApiResponse::ok(state.quality_controller.get_stats()))
}

/// Set auto quality mode
#[derive(serde::Deserialize)]
struct SetAutoQualityReq {
    auto: bool,
}

async fn set_quality_auto_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetAutoQualityReq>,
) -> impl IntoResponse {
    state.quality_controller.set_auto_enabled(req.auto);
    Json(ApiResponse::ok(state.quality_controller.get_stats()))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    // Old protocol (text JSON array): [type, ...ints]
    // New protocol (>= Go 2.3.2): binary frame where first byte is type and remainder is a raw HID report.
    const MSG_HEARTBEAT: i32 = 0;
    const MSG_KEYBOARD: i32 = 1;
    const MSG_MOUSE: i32 = 2;

    // Mouse event subtypes
    const MOUSE_UP: i32 = 0;
    const MOUSE_DOWN: i32 = 1;
    const MOUSE_MOVE_ABSOLUTE: i32 = 2;
    const MOUSE_MOVE_RELATIVE: i32 = 3;
    const MOUSE_SCROLL: i32 = 4;

    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Binary(bin) => {
                if bin.is_empty() {
                    continue;
                }
                // New protocol: first byte is type, rest is raw HID report.
                match bin[0] {
                    0 => {
                        state.jiggler.update_activity();
                    }
                    1 => {
                        state.jiggler.update_activity();
                        let report = &bin[1..];
                        if report.len() == 8 {
                            let mut hid = state.hid.lock().await;
                            let _ = hid.send_keyboard(report).await;
                        }
                    }
                    2 => {
                        state.jiggler.update_activity();
                        let report = &bin[1..];
                        if report.len() == 4 || report.len() == 6 {
                            let mut hid = state.hid.lock().await;
                            let _ = hid.send_mouse(report).await;
                        }
                    }
                    _ => {}
                }
            }
            Message::Text(text) => {
                let event: Vec<i32> = match serde_json::from_str(&text) {
                    Ok(arr) => arr,
                    Err(_) => continue,
                };
                if event.is_empty() {
                    continue;
                }

                match event[0] {
                    MSG_HEARTBEAT => {
                        state.jiggler.update_activity();
                    }
                    MSG_KEYBOARD if event.len() >= 5 => {
                        state.jiggler.update_activity();
                        // Keyboard event: [1, keycode, ctrl, shift, alt, meta]
                        let keycode = event[1] as u8;
                        let modifier = (event[2] as u8)
                            | (event[3] as u8)
                            | (event[4] as u8)
                            | (if event.len() > 5 { event[5] as u8 } else { 0 });

                        let report = [modifier, 0, keycode, 0, 0, 0, 0, 0];
                        let mut hid = state.hid.lock().await;
                        let _ = hid.send_keyboard(&report).await;
                    }
                    MSG_MOUSE if event.len() >= 2 => {
                        state.jiggler.update_activity();
                        let mouse_type = event[1];
                        let mut hid = state.hid.lock().await;

                        match mouse_type {
                            MOUSE_UP => {
                                let _ = hid.send_mouse(&[0u8, 0, 0, 0]).await;
                            }
                            MOUSE_DOWN if event.len() >= 3 => {
                                let button = event[2] as u8;
                                let _ = hid.send_mouse(&[button, 0, 0, 0]).await;
                            }
                            MOUSE_MOVE_ABSOLUTE if event.len() >= 5 => {
                                let x = event[3] as u16;
                                let y = event[4] as u16;
                                let report = [
                                    0,
                                    (x & 0xFF) as u8,
                                    (x >> 8) as u8,
                                    (y & 0xFF) as u8,
                                    (y >> 8) as u8,
                                    0,
                                ];
                                let _ = hid.send_mouse(&report).await;
                            }
                            MOUSE_MOVE_RELATIVE if event.len() >= 5 => {
                                let button = event[2] as u8;
                                let dx = event[3] as i8 as u8;
                                let dy = event[4] as i8 as u8;
                                let _ = hid.send_mouse(&[button, dx, dy, 0]).await;
                            }
                            MOUSE_SCROLL if event.len() >= 5 => {
                                let direction = if event[4] < 0 { 0xFFu8 } else { 0x01u8 };
                                let _ = hid.send_mouse(&[0, 0, 0, direction]).await;
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            _ => continue,
        }
    }

    // Connection closed, release all keys and buttons to prevent "sticky keys"
    debug!("WebSocket closed, releasing HID inputs");
    let mut hid = state.hid.lock().await;
    let _ = hid.send_keyboard(&[0u8; 8]).await;
    let _ = hid.send_mouse(&[0u8; 4]).await;
    let _ = hid.send_mouse(&[0u8; 6]).await;
}
