//! Brute-force protection module (P0 security parity).
//! Clean, proper implementation matching the official Go version's behavior.

use crate::api::error_codes;
use crate::config::Security;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
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
        Self {
            attempts: RwLock::new(HashMap::new()),
            security,
        }
    }

    /// Load persisted lockout state from disk. Call once at startup before serving traffic.
    pub async fn load_state(&self) -> std::io::Result<()> {
        self.load().await
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
        match fs::read_to_string(STATE_FILE).await {
            Ok(data) => match serde_json::from_str::<HashMap<String, LoginAttempt>>(&data) {
                Ok(map) => {
                    *self.attempts.write() = map;
                }
                Err(e) => {
                    warn!("Failed to parse brute force state file: {}", e);
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // First run is normal
            }
            Err(e) => {
                warn!("Failed to read brute force state file: {}", e);
            }
        }
        Ok(())
    }

    async fn save(&self) {
        let map = self.attempts.read().clone();
        match serde_json::to_string(&map) {
            Ok(json) => {
                let tmp = format!("{}.tmp", STATE_FILE);
                let mut builder = OpenOptions::new();
                builder.create(true).write(true).truncate(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    builder.mode(0o600);
                }
                match builder.open(&tmp).await {
                    Ok(mut file) => {
                        if let Err(e) = file.write_all(json.as_bytes()).await {
                            warn!("Failed to write brute force state temp file: {}", e);
                            return;
                        }
                        if let Err(e) = file.sync_all().await {
                            warn!("Failed to sync brute force state temp file: {}", e);
                            return;
                        }
                        if let Err(e) = fs::rename(&tmp, STATE_FILE).await {
                            warn!("Failed to commit brute force state file: {}", e);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to open brute force state temp file: {}", e);
                    }
                }
            }
            Err(e) => {
                warn!("Failed to serialize brute force state: {}", e);
            }
        }
    }

    /// Spawn cleanup task (call once after construction).
    pub fn spawn_cleanup(this: &Arc<Self>) {
        if !this.is_enabled() {
            return;
        }

        let this = Arc::clone(this);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
            loop {
                interval.tick().await;

                let mut map = this.attempts.write();
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();

                let before = map.len();
                map.retain(|_, a| {
                    if a.lockout_end > 0 && now < a.lockout_end {
                        return true;
                    }
                    if a.lockout_end == 0 && now.saturating_sub(a.last_failed) < IDLE_TTL.as_secs()
                    {
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
                    error_codes::LOCKED,
                    "Account locked due to too many failed attempts, please try again later"
                        .to_string(),
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
        let window = self.security.login_lockout_duration as u64;
        let max_failures = self.security.login_max_failures as u32;

        let locked = {
            let mut map = self.attempts.write();

            if map.len() >= MAX_RECORDS {
                map.clear();
            }

            let attempt = map.entry(ip.to_string()).or_default();

            if attempt.last_failed > 0 && now.saturating_sub(attempt.last_failed) > window {
                attempt.failures = 0;
                attempt.lockout_end = 0;
            }

            attempt.failures += 1;
            attempt.last_failed = now;

            let lockout = attempt.failures >= max_failures;
            if lockout {
                attempt.lockout_end = now + window;
            }
            lockout
        };

        self.save().await;

        if locked {
            Some((
                error_codes::LOCKED,
                "Account locked due to too many failed attempts, please try again later"
                    .to_string(),
            ))
        } else {
            None
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error_codes;
    use crate::config::Security;

    fn test_security(duration_secs: i32, max_failures: i32) -> Security {
        Security {
            login_lockout_duration: duration_secs,
            login_max_failures: max_failures,
            trust_forwarded_headers: false,
        }
    }

    #[tokio::test]
    async fn test_basic_lockout_flow() {
        let bf = BruteForce::new(test_security(300, 3)); // 5 min lock after 3 fails

        let ip = "192.0.2.1";

        assert!(bf.check(ip).is_none());

        // First two failures should not lock
        assert!(bf.record_failure(ip).await.is_none());
        assert!(bf.record_failure(ip).await.is_none());

        // Third failure should lock
        let lock = bf.record_failure(ip).await;
        assert!(lock.is_some());
        assert_eq!(lock.unwrap().0, error_codes::LOCKED);

        // Now it should be locked
        let locked = bf.check(ip);
        assert!(locked.is_some());
        assert_eq!(locked.unwrap().0, error_codes::LOCKED);

        // Clear should unlock
        bf.clear(ip).await;
        assert!(bf.check(ip).is_none());
    }

    #[test]
    fn test_disabled_when_zero() {
        let bf = BruteForce::new(test_security(0, 5));
        assert!(!bf.is_enabled());

        let bf2 = BruteForce::new(test_security(300, 0));
        assert!(!bf2.is_enabled());
    }
}
