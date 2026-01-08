use thiserror::Error;
use tokio::fs::{File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::time::{timeout, Duration};
use tracing::{debug, error, info};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShortcutKey {
    pub code: String,
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Shortcut {
    pub id: String,
    pub keys: Vec<ShortcutKey>,
}

#[derive(Error, Debug)]
pub enum HidError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Write Timeout")]
    Timeout,
    #[error("Invalid data length: expected {expected}, got {got}")]
    InvalidLength { expected: usize, got: usize },
}

pub struct HidEngine {
    // We keep file handles open
    keyboard: Option<File>,
    mouse_rel: Option<File>,
    mouse_abs: Option<File>,
}

const HID_KEYBOARD: &str = "/dev/hidg0";
const HID_MOUSE_REL: &str = "/dev/hidg1";
const HID_MOUSE_ABS: &str = "/dev/hidg2";
const WRITE_TIMEOUT: Duration = Duration::from_millis(10); // 8ms in Go, let's allow 10ms

impl HidEngine {
    pub async fn new() -> Self {
        let mut engine = Self {
            keyboard: None,
            mouse_rel: None,
            mouse_abs: None,
        };
        engine.open_all().await;
        engine
    }

    async fn open_device(path: &str) -> Option<File> {
        match OpenOptions::new().write(true).open(path).await {
            Ok(f) => {
                info!("Opened HID device: {}", path);
                Some(f)
            }
            Err(e) => {
                error!("Failed to open {}: {}", path, e);
                None
            }
        }
    }

    pub async fn open_all(&mut self) {
        if self.keyboard.is_none() {
            self.keyboard = Self::open_device(HID_KEYBOARD).await;
        }
        if self.mouse_rel.is_none() {
            self.mouse_rel = Self::open_device(HID_MOUSE_REL).await;
        }
        if self.mouse_abs.is_none() {
            self.mouse_abs = Self::open_device(HID_MOUSE_ABS).await;
        }
    }

    async fn write_with_timeout(
        file: &mut Option<File>,
        path: &str,
        data: &[u8],
    ) -> Result<(), HidError> {
        if let Some(f) = file {
            // Attempt write with timeout
            match timeout(WRITE_TIMEOUT, f.write_all(data)).await {
                Ok(Ok(_)) => {
                    // Success
                    debug!("Wrote to {}: {:?}", path, data);
                    Ok(())
                }
                Ok(Err(e)) => {
                    // IO Error (e.g. device disconnected?)
                    error!("Write error on {}: {}", path, e);
                    // Close/Invalidate handle so we try to reopen next time?
                    // In Go logic, they check for os.ErrClosed and reopen.
                    // Here we might want to return error and let caller handle, or invalidate here.
                    // Simpler to just invalidate.
                    *file = None;
                    Err(HidError::Io(e))
                }
                Err(_) => {
                    // Timeout
                    debug!("Write timeout on {}", path);
                    Err(HidError::Timeout)
                }
            }
        } else {
            // Try to reopen?
            // For now, fail silent-ish to avoid log spam loop, or just return error
            Err(HidError::Io(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "Device not open",
            )))
        }
    }

    pub async fn send_keyboard(&mut self, report: &[u8]) -> Result<(), HidError> {
        if report.len() != 8 {
            return Err(HidError::InvalidLength {
                expected: 8,
                got: report.len(),
            });
        }

        let res = Self::write_with_timeout(&mut self.keyboard, HID_KEYBOARD, report).await;
        if matches!(res, Err(HidError::Io(_))) {
            // Try to reopen immediately once
            self.keyboard = Self::open_device(HID_KEYBOARD).await;
            return Self::write_with_timeout(&mut self.keyboard, HID_KEYBOARD, report).await;
        }
        res
    }

    pub async fn send_mouse(&mut self, report: &[u8]) -> Result<(), HidError> {
        match report.len() {
            4 => {
                let res =
                    Self::write_with_timeout(&mut self.mouse_rel, HID_MOUSE_REL, report).await;
                if matches!(res, Err(HidError::Io(_))) {
                    self.mouse_rel = Self::open_device(HID_MOUSE_REL).await;
                    return Self::write_with_timeout(&mut self.mouse_rel, HID_MOUSE_REL, report)
                        .await;
                }
                res
            }
            6 => {
                let res =
                    Self::write_with_timeout(&mut self.mouse_abs, HID_MOUSE_ABS, report).await;
                if matches!(res, Err(HidError::Io(_))) {
                    self.mouse_abs = Self::open_device(HID_MOUSE_ABS).await;
                    return Self::write_with_timeout(&mut self.mouse_abs, HID_MOUSE_ABS, report)
                        .await;
                }
                res
            }
            len => Err(HidError::InvalidLength {
                expected: 4,
                got: len,
            }), // Or 6
        }
    }
}
