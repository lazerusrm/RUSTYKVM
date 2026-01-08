use hid::HidEngine;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::sync::Mutex;
use tracing::{debug, info};

pub const JIGGLER_CONFIG_FILE: &str = "/etc/kvm/mouse-jiggler";
const JIGGLER_INTERVAL: Duration = Duration::from_secs(15);

pub struct MouseJiggler {
    hid: Arc<Mutex<HidEngine>>,
}

impl MouseJiggler {
    pub fn new(hid: Arc<Mutex<HidEngine>>) -> Self {
        Self { hid }
    }

    pub async fn is_enabled(&self) -> bool {
        fs::metadata(JIGGLER_CONFIG_FILE).await.is_ok()
    }

    pub async fn get_mode(&self) -> String {
        match fs::read_to_string(JIGGLER_CONFIG_FILE).await {
            Ok(content) => content.trim().to_string(),
            Err(_) => "relative".to_string(),
        }
    }

    pub async fn enable(&self, mode: &str) -> Result<(), std::io::Error> {
        fs::write(JIGGLER_CONFIG_FILE, mode).await?;
        Ok(())
    }

    pub async fn disable(&self) -> Result<(), std::io::Error> {
        let _ = fs::remove_file(JIGGLER_CONFIG_FILE).await;
        Ok(())
    }

    pub async fn spawn_loop(&self) {
        let hid = self.hid.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(JIGGLER_INTERVAL);
            loop {
                interval.tick().await;

                if fs::metadata(JIGGLER_CONFIG_FILE).await.is_ok() {
                    let mode = match fs::read_to_string(JIGGLER_CONFIG_FILE).await {
                        Ok(c) => c.trim().to_string(),
                        Err(_) => "relative".to_string(),
                    };

                    debug!("Mouse jiggler: moving mouse ({})", mode);

                    let mut h = hid.lock().await;
                    if mode == "absolute" {
                        // Go: {0x00, 0x00, 0x3f, 0x00, 0x3f, 0x00} then {0x00, 0xff, 0x3f, 0xff, 0x3f, 0x00}
                        let _ = h.send_mouse(&[0x00, 0x00, 0x3f, 0x00, 0x3f, 0x00]).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        let _ = h.send_mouse(&[0x00, 0xff, 0x3f, 0xff, 0x3f, 0x00]).await;
                    } else {
                        // Go: {0x00, 0xa, 0xa, 0x00} then {0x00, 0xf6, 0xf6, 0x00}
                        let _ = h.send_mouse(&[0x00, 0x0a, 0x0a, 0x00]).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        let _ = h.send_mouse(&[0x00, 0xf6, 0xf6, 0x00]).await;
                    }
                }
            }
        });
    }
}
