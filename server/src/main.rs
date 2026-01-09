mod application;
mod auth;
mod config;
mod download;
mod hid;
mod network;
mod passkey;
mod storage;
mod storage_health;
mod tailscale;
mod utils;
mod vm;
mod webrtc;

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
use tokio::signal;
use tokio::sync::{broadcast, Mutex};
use tokio_stream::wrappers::BroadcastStream;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::{debug, error, info};

#[cfg(target_os = "linux")]
use crate::webrtc::screen::{stop_frame_detect_handler, update_frame_detect_handler};

#[cfg(target_os = "linux")]
use crate::vm::{
    delete_autostart_handler, delete_script_handler, disable_hdmi_handler, disable_mdns_handler,
    disable_ssh_handler, enable_hdmi_handler, enable_mdns_handler, enable_ssh_handler,
    get_autostart_content_handler, get_autostart_handler, get_gpio_handler, get_hardware_handler,
    get_hdmi_state_handler, get_hostname_handler, get_info_handler, get_jiggler_handler,
    get_mdns_handler, get_oled_handler, get_scripts_handler, get_ssh_handler, get_swap_handler,
    get_virtual_device_handler, get_web_title_handler, reset_hdmi_handler, run_script_handler,
    set_gpio_handler, set_hostname_handler, set_jiggler_handler, set_oled_handler,
    set_screen_handler, set_swap_handler, set_tls_handler, set_web_title_handler, terminal_handler,
    update_virtual_device_handler, upload_autostart_handler, upload_script_handler,
};

use crate::application::{
    get_preview_handler, get_version_handler, offline_update_handler, set_preview_handler,
    update_handler,
};
use crate::auth::{
    auth_middleware, change_password_handler, get_account_handler, is_password_updated_handler,
    login_handler, logout_handler,
};
use crate::config::Config;
use crate::hid::{
    add_shortcut_handler, delete_shortcut_handler, get_hid_mode_handler, get_shortcuts_handler,
    paste_handler, reset_hid_handler, set_hid_mode_handler,
};
use crate::network::{
    connect_wifi_handler, delete_wol_mac_handler, disconnect_wifi_handler, get_wifi_handler,
    get_wol_macs_handler, set_wol_name_handler, wol_handler,
};
use crate::passkey::handlers::{
    enroll_complete_handler, login_challenge_handler, login_verify_handler, passkey_setup_handler,
    qr_code_handler, recover_handler, recovery_download_handler,
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
    tailscale_down_handler, tailscale_install_handler, tailscale_login_handler,
    tailscale_logout_handler, tailscale_start_handler, tailscale_status_handler,
    tailscale_stop_handler, tailscale_uninstall_handler, tailscale_up_handler,
};
#[cfg(not(target_os = "linux"))]
use crate::vm::{
    delete_script_handler, get_info_handler, get_oled_handler, get_scripts_handler,
    run_script_handler, set_oled_handler, terminal_handler, upload_script_handler,
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

// Shared application state
#[cfg(target_os = "linux")]
pub struct AppState {
    config: Arc<Config>,
    screen_config: crate::webrtc::screen::SharedScreenConfig,
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
}

#[cfg(not(target_os = "linux"))]
pub struct AppState {
    config: Arc<Config>,
    screen_config: crate::webrtc::screen::SharedScreenConfig,
    tx_mjpeg: broadcast::Sender<Bytes>,
    tx_h264: broadcast::Sender<H264Frame>,
    tx_audio: broadcast::Sender<Bytes>,
    hid: Arc<Mutex<::hid::HidEngine>>,
    webrtc: Arc<PeerConnectionManager>,
    whep: Arc<WhepEndpoint>,
    health_state: Arc<HealthState>,
    passkey_state: Arc<crate::passkey::PasskeyState>,
}

#[tokio::main]
async fn main() {
    // Create shutdown broadcast channel
    let (_shutdown_tx, _) = broadcast::channel::<()>(1);

    // 1. Load Configuration
    let config = Arc::new(Config::load().await);

    // 2. Initialize Logging
    let _guard = init_logging(&config);
    info!("NanoKVM Rust Server Starting...");

    let screen_config = Arc::new(parking_lot::RwLock::new(
        crate::webrtc::screen::ScreenConfig::new(),
    ));

    // 3. Initialize Hardware & Loops
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
    let hid_engine = Arc::new(Mutex::new(::hid::HidEngine::new().await));

    // Create Broadcast Channels
    let (tx_mjpeg, _rx) = broadcast::channel::<Bytes>(16);
    let (tx_h264, _rx) = broadcast::channel::<H264Frame>(16);
    let (tx_audio, _rx) = broadcast::channel::<Bytes>(16);

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

    #[cfg(target_os = "linux")]
    let shared_state = Arc::new(AppState {
        config: config.clone(),
        screen_config: screen_config.clone(),
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
    });

    #[cfg(not(target_os = "linux"))]
    let shared_state = Arc::new(AppState {
        config: config.clone(),
        screen_config: screen_config.clone(),
        tx_mjpeg,
        tx_h264: tx_h264.clone(),
        tx_audio: tx_audio.clone(),
        hid: hid_engine.clone(),
        webrtc: webrtc_manager.clone(),
        whep: whep_endpoint.clone(),
        health_state: Arc::new(HealthState::default()),
        passkey_state,
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

    let web_path = "web";

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
        .route("/vm/script/run", post(run_script_handler))
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
        .route(
            "/network/wifi",
            get(get_wifi_handler)
                .post(connect_wifi_handler)
                .delete(disconnect_wifi_handler),
        )
        .route("/ws", get(ws_handler));

    #[cfg(target_os = "linux")]
    {
        api_routes = api_routes
            .route("/stream/mjpeg/detect", post(update_frame_detect_handler))
            .route("/stream/mjpeg/detect/stop", post(stop_frame_detect_handler));
    }

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
                "/vm/autostart/:name",
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
            .route("/tailscale/install", post(tailscale_install_handler))
            .route("/tailscale/uninstall", post(tailscale_uninstall_handler))
            .route("/tailscale/start", post(tailscale_start_handler))
            .route("/tailscale/stop", post(tailscale_stop_handler))
            .route("/tailscale/status", get(tailscale_status_handler))
            .route("/tailscale/login", post(tailscale_login_handler))
            .route("/tailscale/up", post(tailscale_up_handler))
            .route("/tailscale/down", post(tailscale_down_handler))
            .route("/tailscale/logout", post(tailscale_logout_handler));
    }

    let api_routes = api_routes
        .route("/webrtc/whep", post(whep_post_handler))
        .route(
            "/webrtc/whep/:id",
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
        .route("/api/login", post(login_handler))
        .route("/api/logout", post(logout_handler))
        .route("/api/auth/login", post(login_handler))
        .route("/api/auth/account", get(get_account_handler))
        .route(
            "/api/auth/password",
            get(is_password_updated_handler).post(change_password_handler),
        )
        .route("/api/auth/logout", post(logout_handler))
        // Passkey authentication routes (unauthenticated)
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
        .nest("/api", api_routes)
        .nest_service("/", ServeDir::new(web_path))
        .layer(CompressionLayer::new())
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
                let listener = tokio::net::TcpListener::bind(&http_addr)
                    .await
                    .expect("HTTP bind failed");
                info!("Server listening on {} (HTTP - TLS fallback)", http_addr);
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
            let listener = tokio::net::TcpListener::bind(&http_addr)
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
        let listener = tokio::net::TcpListener::bind(&http_addr)
            .await
            .expect("HTTP bind failed");
        info!("Server listening on {} (HTTP)", http_addr);
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
        let (width, height, quality, fps) = {
            let cfg = state.screen_config.read();
            (cfg.width, cfg.height, cfg.quality, cfg.fps)
        };
        interval.tick().await;
        if state.tx_mjpeg.receiver_count() > 0 {
            let kvm_state = state.clone();
            if let Ok(Ok(frame)) =
                tokio::task::spawn_blocking(move || kvm_state.kvm.get_mjpeg(width, height, quality))
                    .await
            {
                let _ = state.tx_mjpeg.send(frame.into_bytes());
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
    loop {
        let (width, height, bitrate, fps) = {
            let cfg = state.screen_config.read();
            (cfg.width, cfg.height, cfg.bitrate, cfg.fps)
        };
        interval.tick().await;
        if state.webrtc.total_connection_count() > 0 || state.tx_h264.receiver_count() > 0 {
            let kvm_state = state.clone();
            if let Ok(Ok(frame_result)) =
                tokio::task::spawn_blocking(move || kvm_state.kvm.get_h264(width, height, bitrate))
                    .await
            {
                let frame_data = frame_result.into_bytes();
                let is_keyframe = !frame_data.is_empty()
                    && (frame_data[0] & 0x1F == 7
                        || (frame_data.len() > 4 && frame_data[4] & 0x1F == 7));
                let timestamp = start_time.elapsed().as_micros() as u64;
                let packets = Arc::new(crate::webrtc::transport::packetize_h264_optimized(
                    &frame_data,
                ));
                let h264_frame = H264Frame {
                    is_keyframe,
                    timestamp,
                    packets: packets.clone(),
                    raw_data: frame_data,
                };
                let _ = state.tx_h264.send(h264_frame);
                let conn_ids = state.webrtc.get_source_connections("default");
                let _ = state
                    .webrtc
                    .broadcast_frame(conn_ids, pts as u32, &packets)
                    .await;
                pts = pts.wrapping_add(3000);
            }
        }
        let frame_interval = Duration::from_secs_f64(1.0 / fps as f64);
        if interval.period() != frame_interval {
            interval = tokio::time::interval(frame_interval);
        }
    }
}

async fn mjpeg_stream(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rx = state.tx_mjpeg.subscribe();
    let s = BroadcastStream::new(rx)
        .filter_map(|r| async move { r.ok() })
        .map(|f| {
            let h = format!(
                "--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n",
                f.len()
            );
            let mut m = Vec::with_capacity(h.len() + f.len() + 2);
            m.extend_from_slice(h.as_bytes());
            m.extend_from_slice(&f);
            m.extend_from_slice(b"\r\n");
            Ok::<Bytes, axum::Error>(Bytes::from(m))
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
    let mut rx = state.tx_h264.subscribe();
    while let Ok(f) = rx.recv().await {
        let mut m = Vec::with_capacity(9 + f.raw_data.len());
        m.push(if f.is_keyframe { 1 } else { 0 });
        m.extend_from_slice(&f.timestamp.to_le_bytes());
        m.extend_from_slice(&f.raw_data);
        if socket.send(Message::Binary(m.into())).await.is_err() {
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
    Json(serde_json::json!({
        "status": "ok",
        "service": "nanokvm"
    }))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    const MSG_HEARTBEAT: u8 = 0;
    const MSG_KEYBOARD: u8 = 1;
    const MSG_MOUSE: u8 = 2;

    while let Some(Ok(msg)) = socket.recv().await {
        if let Message::Binary(data) = msg {
            if data.is_empty() {
                continue;
            }
            let (msg_type, payload) = (data[0], &data[1..]);
            match msg_type {
                MSG_HEARTBEAT => {}
                MSG_KEYBOARD => {
                    let mut hid = state.hid.lock().await;
                    let _ = hid.send_keyboard(payload).await;
                }
                MSG_MOUSE => {
                    let mut hid = state.hid.lock().await;
                    let _ = hid.send_mouse(payload).await;
                }
                _ => {}
            }
        }
    }

    // Connection closed, release all keys and buttons to prevent "sticky keys"
    debug!("WebSocket closed, releasing HID inputs");
    let mut hid = state.hid.lock().await;
    let _ = hid.send_keyboard(&[0u8; 8]).await;
    let _ = hid.send_mouse(&[0u8; 4]).await;
    let _ = hid.send_mouse(&[0u8; 6]).await;
}
