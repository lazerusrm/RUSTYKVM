//! Brute-force protection module (P0 security parity).
//! Clean, proper implementation matching the official Go version's behavior.

use crate::config::Security;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tracing::{debug, warn};

const STATE_FILE: &str = "/etc/kvm/brute_force_state.json";
const MAX_RECORDS: usize = 3000;
const CLEANUP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const IDLE_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LoginAttempt {
    failures: u32,
    last_failed: u64,
    lockout_end: u64,
}

pub struct BruteForce {
    attempts: RwLock<HashMap<String, LoginAttempt>>,
    security: Security,
}

impl BruteForce {
    pub fn new(security: Security) -> Self {
        let this = Self {
            attempts: RwLock::new(HashMap::new()),
            security,
        };
        // Best effort load from disk
        let _ = this.load();
        this
    }

    fn is_enabled(&self) -> bool {
        self.security.login_lockout_duration > 0 && self.security.login_max_failures > 0
    }

    fn now(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    async fn load(&self) -> std::io::Result<()> {
        if let Ok(data) = fs::read_to_string(STATE_FILE).await {
            if let Ok(map) = serde_json::from_str::<HashMap<String, LoginAttempt>>(&data) {
                *self.attempts.write() = map;
            }
        }
        Ok(())
    }

    async fn save(&self) {
        let map = self.attempts.read().clone();
        if let Ok(json) = serde_json::to_string(&map) {
            let _ = fs::write(STATE_FILE, json).await;
        }
    }

    /// Spawn cleanup task (call once after construction).
    pub fn spawn_cleanup(&self) {
        if !self.is_enabled() {
            return;
        }

        let attempts = self.attempts.clone();
        let policy = self.policy.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
            loop {
                interval.tick().await;

                let mut map = attempts.write();
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let before = map.len();
                map.retain(|_, a| {
                    if a.lockout_end > 0 && now < a.lockout_end {
                        return true;
                    }
                    if a.lockout_end == 0 && now.saturating_sub(a.last_failed) < IDLE_TTL.as_secs() {
                        return true;
                    }
                    false
                });

                if map.len() < before {
                    debug!("Brute force cleanup: {} -> {} records", before, map.len());
                }

                if map.len() > MAX_RECORDS {
                    warn!("Brute force record cap exceeded, clearing");
                    map.clear();
                }
            }
        });
    }

    pub fn check(&self, ip: &str) -> Option<(i32, String)> {
        if !self.is_enabled() {
            return None;
        }

        let now = self.now();
        let map = self.attempts.read();

        if let Some(attempt) = map.get(ip) {
            if attempt.lockout_end > 0 && now < attempt.lockout_end {
                return Some((
                    -5,
                    "Account locked due to too many failed attempts, please try again later".to_string(),
                ));
            }
        }
        None
    }

    pub async fn record_failure(&self, ip: &str) -> Option<(i32, String)> {
        if !self.is_enabled() {
            return None;
        }

        let now = self.now();
        let mut map = self.attempts.write();

        if map.len() >= MAX_RECORDS {
            map.clear();
        }

        let window = self.security.login_lockout_duration as u64; // already in seconds, Go-style

        let attempt = map.entry(ip.to_string()).or_default();

        if attempt.last_failed > 0 && now.saturating_sub(attempt.last_failed) > window {
            attempt.failures = 0;
            attempt.lockout_end = 0;
        }

        attempt.failures += 1;
        attempt.last_failed = now;

        if attempt.failures >= self.security.login_max_failures as u32 {
            attempt.lockout_end = now + window;
            drop(map); // release lock before await
            self.save().await;
            return Some((
                -5,
                "Account locked due to too many failed attempts, please try again later".to_string(),
            ));
        }

        drop(map);
        self.save().await;
        None
    }

    pub async fn clear(&self, ip: &str) {
        if !self.is_enabled() {
            return;
        }
        {
            let mut map = self.attempts.write();
            map.remove(ip);
        }
        self.save().await;
    }
}
