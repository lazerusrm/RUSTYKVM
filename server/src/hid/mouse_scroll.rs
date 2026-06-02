//! Mouse wheel direction + speed (scroll profile) server-managed configuration.
//!
//! This is an optional enhancement over the official Go implementation (which is
//! 100% client-side in localStorage + Jotai). We expose a small REST surface so
//! that API clients, scripts, and future multi-client UIs can read/write a
//! central profile that survives browser clears and works across devices.
//!
//! The WebSocket scroll path (MOUSE_SCROLL) remains a pure delta passthrough —
//! exactly as in Go and as it was in Rust before this change. No behavior
//! change for existing browser clients.
//!
//! Storage: /etc/kvm/mouse_scroll.json (versioned, atomic writes).
//! Defaults: direction = -1, interval = 0 (match official frontend Jotai defaults).

use crate::api::{error_codes, ApiResponse};
use axum::{
    extract::{Json, State},
    response::IntoResponse,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, warn};

/// File location (consistent with other KVM runtime state: shortcuts, brute force, etc.)
const MOUSE_SCROLL_FILE: &str = "/etc/kvm/mouse_scroll.json";

/// The profile that controls how wheel events are interpreted by clients.
/// Direction:  1 = natural, -1 = inverted (matches Go frontend convention).
/// Interval:   minimum milliseconds between emitted scroll events (0 = no artificial throttle).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MouseScrollConfig {
    pub direction: i8,
    pub interval: u32,
}

impl Default for MouseScrollConfig {
    fn default() -> Self {
        // Exact match to official frontend defaults (jotai/mouse.ts + localstorage.ts)
        Self {
            direction: -1,
            interval: 0,
        }
    }
}

/// Internal on-disk wrapper (versioned for future migrations).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MouseScrollStoreFile {
    version: u32,
    #[serde(flatten)]
    config: MouseScrollConfig,
}

const CURRENT_VERSION: u32 = 1;

/// Thread-safe, file-backed store. Cheap reads via parking_lot RwLock.
/// Writes are validated, applied in-memory, then persisted atomically.
pub struct MouseScrollStore {
    config: RwLock<MouseScrollConfig>,
}

impl MouseScrollStore {
    pub fn new() -> Arc<Self> {
        let store = Arc::new(Self {
            config: RwLock::new(MouseScrollConfig::default()),
        });

        // Best-effort load from disk at startup (non-fatal).
        let store_clone = store.clone();
        tokio::spawn(async move {
            if let Err(e) = store_clone.load_from_disk().await {
                // Only warn — we keep the safe defaults in memory.
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!("mouse_scroll: failed to load {}: {}", MOUSE_SCROLL_FILE, e);
                }
            } else {
                debug!("mouse_scroll: loaded profile from disk");
            }
        });

        store
    }

    async fn load_from_disk(&self) -> std::io::Result<()> {
        let content = fs::read_to_string(MOUSE_SCROLL_FILE).await?;
        if content.trim().is_empty() {
            return Ok(());
        }

        match serde_json::from_str::<MouseScrollStoreFile>(&content) {
            Ok(stored) => {
                if let Err(e) = Self::validate(&stored.config) {
                    warn!("mouse_scroll: rejecting invalid persisted config: {}", e);
                    return Ok(()); // keep defaults
                }
                *self.config.write() = stored.config;
            }
            Err(e) => {
                // Try legacy bare config (no wrapper) for robustness during transition
                if let Ok(cfg) = serde_json::from_str::<MouseScrollConfig>(&content) {
                    if Self::validate(&cfg).is_ok() {
                        *self.config.write() = cfg;
                        return Ok(());
                    }
                }
                warn!("mouse_scroll: failed to parse {}: {}", MOUSE_SCROLL_FILE, e);
            }
        }
        Ok(())
    }

    /// Atomic write: write to .tmp then rename. Restrictive permissions.
    async fn save_to_disk(&self, cfg: &MouseScrollConfig) -> std::io::Result<()> {
        let wrapped = MouseScrollStoreFile {
            version: CURRENT_VERSION,
            config: cfg.clone(),
        };
        let json = serde_json::to_string_pretty(&wrapped)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let tmp = format!("{}.tmp", MOUSE_SCROLL_FILE);

        // Create with 0o600 (owner rw only)
        let builder = {
            let mut b = tokio::fs::OpenOptions::new();
            b.create(true);
            b.write(true);
            b.truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                b.mode(0o600);
            }
            b
        };
        let mut file = builder.open(&tmp).await?;

        file.write_all(json.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await?;
        drop(file);

        fs::rename(&tmp, MOUSE_SCROLL_FILE).await
    }

    fn validate(cfg: &MouseScrollConfig) -> Result<(), String> {
        if cfg.direction != 1 && cfg.direction != -1 {
            return Err("direction must be 1 or -1".to_string());
        }
        if cfg.interval > 2000 {
            return Err("interval must be between 0 and 2000 (ms)".to_string());
        }
        Ok(())
    }

    /// Cheap, lock-free-for-readers snapshot.
    pub fn get(&self) -> MouseScrollConfig {
        self.config.read().clone()
    }

    /// Validate + apply + persist. Returns the stored config on success.
    pub async fn set(
        &self,
        new_cfg: MouseScrollConfig,
    ) -> Result<MouseScrollConfig, (i32, String)> {
        if let Err(msg) = Self::validate(&new_cfg) {
            return Err((error_codes::VALIDATION, msg));
        }

        {
            let mut guard = self.config.write();
            *guard = new_cfg.clone();
        }

        if let Err(e) = self.save_to_disk(&new_cfg).await {
            warn!("mouse_scroll: save failed: {}", e);
            // We still keep the in-memory value (best effort persistence).
            // Caller sees a generic error but the change is live for this process.
            return Err((
                error_codes::GENERIC,
                "profile updated in memory but failed to persist to disk".to_string(),
            ));
        }

        debug!(
            "mouse_scroll: profile updated direction={} interval={}",
            new_cfg.direction, new_cfg.interval
        );
        Ok(new_cfg)
    }
}

// -----------------------------
// HTTP DTOs (match design + frontend naming)
// -----------------------------

#[derive(Debug, Serialize)]
pub struct GetMouseScrollRsp {
    pub direction: i8,
    pub interval: u32,
    /// "server" when we have a persisted value (always true once loaded)
    pub source: String,
}

#[derive(Debug, Deserialize)]
pub struct SetMouseScrollReq {
    pub direction: i8,
    pub interval: u32,
}

// -----------------------------
// Handlers (registered under the authenticated api router)
// -----------------------------

pub async fn get_mouse_scroll_handler(
    State(state): State<Arc<crate::AppState>>,
) -> impl IntoResponse {
    let cfg = state.mouse_scroll.get();
    Json(ApiResponse::ok(GetMouseScrollRsp {
        direction: cfg.direction,
        interval: cfg.interval,
        source: "server".to_string(),
    }))
}

pub async fn set_mouse_scroll_handler(
    State(state): State<Arc<crate::AppState>>,
    Json(req): Json<SetMouseScrollReq>,
) -> impl IntoResponse {
    let new_cfg = MouseScrollConfig {
        direction: req.direction,
        interval: req.interval,
    };

    match state.mouse_scroll.set(new_cfg).await {
        Ok(stored) => Json(ApiResponse::ok(GetMouseScrollRsp {
            direction: stored.direction,
            interval: stored.interval,
            source: "server".to_string(),
        }))
        .into_response(),
        Err((code, msg)) => Json(ApiResponse::<serde_json::Value>::err(code, &msg)).into_response(),
    }
}
