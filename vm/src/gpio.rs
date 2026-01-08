use crate::{HardwareConfig, HardwareVersion};
use std::time::Duration;
use thiserror::Error;
use tokio::fs;
use tracing::{debug, error};

#[derive(Error, Debug)]
pub enum GpioError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid GPIO state value: {0}")]
    InvalidState(String),
}

pub struct VmController {
    config: HardwareConfig,
}

impl VmController {
    pub async fn new() -> Self {
        let version = HardwareVersion::detect().await;
        let config = HardwareConfig::get(version);
        Self { config }
    }

    pub fn get_version(&self) -> HardwareVersion {
        self.config.version
    }

    pub async fn power_press(&self, duration_ms: u64) -> Result<(), GpioError> {
        self.pulse_gpio(self.config.gpio_power, duration_ms).await
    }

    pub async fn reset_press(&self, duration_ms: u64) -> Result<(), GpioError> {
        self.pulse_gpio(self.config.gpio_reset, duration_ms).await
    }

    async fn pulse_gpio(&self, path: &str, duration_ms: u64) -> Result<(), GpioError> {
        debug!("Pulsing GPIO {} for {}ms", path, duration_ms);

        // Press (Set to 1)
        fs::write(path, b"1").await.map_err(|e| {
            error!("Failed to write 1 to {}: {}", path, e);
            e
        })?;

        tokio::time::sleep(Duration::from_millis(duration_ms)).await;

        // Release (Set to 0)
        fs::write(path, b"0").await.map_err(|e| {
            error!("Failed to write 0 to {}: {}", path, e);
            e
        })?;

        Ok(())
    }

    pub async fn get_power_led(&self) -> Result<bool, GpioError> {
        self.read_gpio(self.config.gpio_power_led).await
    }

    pub async fn get_hdd_led(&self) -> Result<bool, GpioError> {
        if let Some(path) = self.config.gpio_hdd_led {
            self.read_gpio(path).await
        } else {
            Ok(false)
        }
    }

    async fn read_gpio(&self, path: &str) -> Result<bool, GpioError> {
        let content = fs::read_to_string(path).await.map_err(|e| {
            error!("Failed to read from {}: {}", path, e);
            e
        })?;

        let val = content.trim();
        // LED indicators use active-low signaling (common hardware design).
        // When GPIO value is 0, the LED circuit is complete and the LED is ON.
        // When GPIO value is 1, the LED circuit is open and the LED is OFF.
        Ok(val == "0")
    }
}
