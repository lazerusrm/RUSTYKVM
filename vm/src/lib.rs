pub mod gpio;
pub mod jiggler;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HardwareVersion {
    Alpha,
    Beta,
    Pcie,
}

impl HardwareVersion {
    pub async fn detect() -> Self {
        match tokio::fs::read_to_string("/etc/kvm/hw").await {
            Ok(content) => match content.trim() {
                "alpha" => HardwareVersion::Alpha,
                "beta" => HardwareVersion::Beta,
                "pcie" => HardwareVersion::Pcie,
                _ => HardwareVersion::Alpha,
            },
            Err(_) => HardwareVersion::Alpha,
        }
    }
}

pub struct HardwareConfig {
    pub version: HardwareVersion,
    pub gpio_reset: &'static str,
    pub gpio_power: &'static str,
    pub gpio_power_led: &'static str,
    pub gpio_hdd_led: Option<&'static str>,
}

impl HardwareConfig {
    pub fn get(version: HardwareVersion) -> Self {
        match version {
            HardwareVersion::Alpha => HardwareConfig {
                version,
                gpio_reset: "/sys/class/gpio/gpio507/value",
                gpio_power: "/sys/class/gpio/gpio503/value",
                gpio_power_led: "/sys/class/gpio/gpio504/value",
                gpio_hdd_led: Some("/sys/class/gpio/gpio505/value"),
            },
            HardwareVersion::Beta => HardwareConfig {
                version,
                gpio_reset: "/sys/class/gpio/gpio505/value",
                gpio_power: "/sys/class/gpio/gpio503/value",
                gpio_power_led: "/sys/class/gpio/gpio504/value",
                gpio_hdd_led: None,
            },
            HardwareVersion::Pcie => HardwareConfig {
                version,
                gpio_reset: "/sys/class/gpio/gpio505/value",
                gpio_power: "/sys/class/gpio/gpio503/value",
                gpio_power_led: "/sys/class/gpio/gpio504/value",
                gpio_hdd_led: None,
            },
        }
    }
}
