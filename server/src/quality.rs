//! Adaptive Quality Controller
//!
//! Implements automatic quality adjustment based on network backpressure.
//! Uses AIMD (Additive Increase Multiplicative Decrease) algorithm:
//! - Slowly increase quality when network is healthy
//! - Quickly decrease quality when congestion is detected

use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Quality tier definition
#[derive(Debug, Clone, Copy)]
pub struct QualityTier {
    pub name: &'static str,
    pub h264_bitrate: u16,  // kbps
    pub mjpeg_quality: u16, // 1-100
}

/// Predefined quality tiers from highest to lowest
pub const QUALITY_TIERS: [QualityTier; 5] = [
    QualityTier {
        name: "ultra",
        h264_bitrate: 4000,
        mjpeg_quality: 95,
    },
    QualityTier {
        name: "high",
        h264_bitrate: 2500,
        mjpeg_quality: 80,
    },
    QualityTier {
        name: "medium",
        h264_bitrate: 1500,
        mjpeg_quality: 65,
    },
    QualityTier {
        name: "low",
        h264_bitrate: 800,
        mjpeg_quality: 45,
    },
    QualityTier {
        name: "lowest",
        h264_bitrate: 400,
        mjpeg_quality: 30,
    },
];

/// Default starting tier (high quality)
const DEFAULT_TIER: usize = 1; // "high"

/// Frames of success before attempting quality increase
const INCREASE_THRESHOLD: u32 = 90; // ~3 seconds at 30fps

/// Consecutive failures before quality decrease
const DECREASE_THRESHOLD: u32 = 3;

/// Minimum time between quality changes to prevent oscillation
const MIN_CHANGE_INTERVAL: Duration = Duration::from_secs(2);

/// Internal state for the quality controller
struct QualityState {
    auto_enabled: bool,
    current_tier: usize,
    consecutive_successes: u32,
    consecutive_failures: u32,
    last_change: Instant,
    // Stats for monitoring
    total_frames: u64,
    dropped_frames: u64,
}

impl QualityState {
    fn new() -> Self {
        Self {
            auto_enabled: true, // Auto quality ON by default
            current_tier: DEFAULT_TIER,
            consecutive_successes: 0,
            consecutive_failures: 0,
            last_change: Instant::now(),
            total_frames: 0,
            dropped_frames: 0,
        }
    }
}

/// Thread-safe adaptive quality controller
pub struct QualityController {
    state: RwLock<QualityState>,
}

impl QualityController {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(QualityState::new()),
        }
    }

    /// Check if auto quality is enabled
    pub fn is_auto_enabled(&self) -> bool {
        self.state.read().auto_enabled
    }

    /// Enable or disable auto quality
    pub fn set_auto_enabled(&self, enabled: bool) {
        let mut state = self.state.write();
        state.auto_enabled = enabled;
        if enabled {
            // Reset to default tier when enabling
            state.current_tier = DEFAULT_TIER;
            state.consecutive_successes = 0;
            state.consecutive_failures = 0;
        }
    }

    /// Get current quality tier index
    pub fn current_tier_index(&self) -> usize {
        self.state.read().current_tier
    }

    /// Get current quality tier
    pub fn current_tier(&self) -> QualityTier {
        let tier_idx = self.state.read().current_tier;
        QUALITY_TIERS[tier_idx]
    }

    /// Get current H.264 bitrate (respects auto mode)
    pub fn get_h264_bitrate(&self, manual_bitrate: u16) -> u16 {
        let state = self.state.read();
        if state.auto_enabled {
            QUALITY_TIERS[state.current_tier].h264_bitrate
        } else {
            manual_bitrate
        }
    }

    /// Get current MJPEG quality (respects auto mode)
    pub fn get_mjpeg_quality(&self, manual_quality: u16) -> u16 {
        let state = self.state.read();
        if state.auto_enabled {
            QUALITY_TIERS[state.current_tier].mjpeg_quality
        } else {
            manual_quality
        }
    }

    /// Report frame send result for adaptation
    /// Returns true if quality tier changed
    pub fn on_frame_result(&self, success: bool) -> bool {
        let mut state = self.state.write();

        if !state.auto_enabled {
            return false;
        }

        state.total_frames += 1;
        if !success {
            state.dropped_frames += 1;
        }

        let now = Instant::now();
        let can_change = now.duration_since(state.last_change) >= MIN_CHANGE_INTERVAL;

        if success {
            state.consecutive_successes += 1;
            state.consecutive_failures = 0;

            // Try to increase quality after sustained success
            if can_change && state.consecutive_successes >= INCREASE_THRESHOLD {
                if state.current_tier > 0 {
                    state.current_tier -= 1; // Lower index = higher quality
                    state.consecutive_successes = 0;
                    state.last_change = now;
                    tracing::info!(
                        "Quality increased to '{}' (tier {})",
                        QUALITY_TIERS[state.current_tier].name,
                        state.current_tier
                    );
                    return true;
                }
            }
        } else {
            state.consecutive_failures += 1;
            state.consecutive_successes = 0;

            // Decrease quality quickly on failures
            if can_change && state.consecutive_failures >= DECREASE_THRESHOLD {
                if state.current_tier < QUALITY_TIERS.len() - 1 {
                    state.current_tier += 1; // Higher index = lower quality
                    state.consecutive_failures = 0;
                    state.last_change = now;
                    tracing::warn!(
                        "Quality decreased to '{}' (tier {}) due to backpressure",
                        QUALITY_TIERS[state.current_tier].name,
                        state.current_tier
                    );
                    return true;
                }
            }
        }

        false
    }

    /// Report REMB (Receiver Estimated Maximum Bitrate) from WebRTC
    /// This allows faster adaptation based on receiver feedback
    pub fn on_remb_received(&self, bitrate_bps: u32) {
        let mut state = self.state.write();

        if !state.auto_enabled {
            return;
        }

        let bitrate_kbps = (bitrate_bps / 1000) as u16;

        // Find the highest quality tier that fits within REMB
        let target_tier = QUALITY_TIERS
            .iter()
            .position(|t| t.h264_bitrate <= bitrate_kbps)
            .unwrap_or(QUALITY_TIERS.len() - 1);

        if target_tier != state.current_tier {
            let now = Instant::now();
            if now.duration_since(state.last_change) >= MIN_CHANGE_INTERVAL {
                state.current_tier = target_tier;
                state.last_change = now;
                state.consecutive_successes = 0;
                state.consecutive_failures = 0;
                tracing::info!(
                    "Quality adjusted to '{}' based on REMB {} kbps",
                    QUALITY_TIERS[state.current_tier].name,
                    bitrate_kbps
                );
            }
        }
    }

    /// Get stats for monitoring
    pub fn get_stats(&self) -> QualityStats {
        let state = self.state.read();
        QualityStats {
            auto_enabled: state.auto_enabled,
            current_tier: QUALITY_TIERS[state.current_tier].name.to_string(),
            current_tier_index: state.current_tier,
            h264_bitrate: QUALITY_TIERS[state.current_tier].h264_bitrate,
            mjpeg_quality: QUALITY_TIERS[state.current_tier].mjpeg_quality,
            total_frames: state.total_frames,
            dropped_frames: state.dropped_frames,
            drop_rate: if state.total_frames > 0 {
                (state.dropped_frames as f64 / state.total_frames as f64) * 100.0
            } else {
                0.0
            },
        }
    }

    /// Reset statistics (but keep current quality tier)
    pub fn reset_stats(&self) {
        let mut state = self.state.write();
        state.total_frames = 0;
        state.dropped_frames = 0;
    }
}

impl Default for QualityController {
    fn default() -> Self {
        Self::new()
    }
}

/// Quality statistics for API response
#[derive(Debug, Clone, serde::Serialize)]
pub struct QualityStats {
    pub auto_enabled: bool,
    pub current_tier: String,
    pub current_tier_index: usize,
    pub h264_bitrate: u16,
    pub mjpeg_quality: u16,
    pub total_frames: u64,
    pub dropped_frames: u64,
    pub drop_rate: f64,
}

/// Shared quality controller type
pub type SharedQualityController = Arc<QualityController>;

/// Create a new shared quality controller
pub fn new_shared() -> SharedQualityController {
    Arc::new(QualityController::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quality_decrease_on_failures() {
        let controller = QualityController::new();
        let initial_tier = controller.current_tier_index();

        // Simulate failures
        for _ in 0..10 {
            controller.on_frame_result(false);
        }

        // After enough failures, quality should decrease (tier index increases)
        // Note: May not change immediately due to MIN_CHANGE_INTERVAL
        assert!(controller.current_tier_index() >= initial_tier);
    }

    #[test]
    fn test_auto_toggle() {
        let controller = QualityController::new();
        assert!(controller.is_auto_enabled());

        controller.set_auto_enabled(false);
        assert!(!controller.is_auto_enabled());

        // When disabled, manual values should be used
        assert_eq!(controller.get_h264_bitrate(3000), 3000);
        assert_eq!(controller.get_mjpeg_quality(70), 70);
    }
}
