use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::fs;
use tracing::info;
use walkdir::WalkDir;

pub mod health;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("IO Error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid filename: {0}")]
    InvalidFilename(String),
}

const IMAGE_DIRECTORY: &str = "/data";
const IMAGE_NONE: &str = "/dev/mmcblk0p3";
const CDROM_FLAG: &str =
    "/sys/kernel/config/usb_gadget/g0/functions/mass_storage.disk0/lun.0/cdrom";
const MOUNT_DEVICE: &str =
    "/sys/kernel/config/usb_gadget/g0/functions/mass_storage.disk0/lun.0/file";
const INQUIRY_STRING: &str =
    "/sys/kernel/config/usb_gadget/g0/functions/mass_storage.disk0/lun.0/inquiry_string";
const RO_FLAG: &str = "/sys/kernel/config/usb_gadget/g0/functions/mass_storage.disk0/lun.0/ro";
const UDC_PATH: &str = "/sys/kernel/config/usb_gadget/g0/UDC";
const UDC_LIST_PATH: &str = "/sys/class/udc/";

pub struct StorageManager;

impl StorageManager {
    pub fn get_images() -> Result<Vec<PathBuf>, StorageError> {
        let mut images = Vec::new();
        for entry in WalkDir::new(IMAGE_DIRECTORY)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if ext_str == "iso" || ext_str == "img" {
                        images.push(path.to_path_buf());
                    }
                }
            }
        }
        Ok(images)
    }

    pub async fn mount_image(file_path: Option<&str>, cdrom: bool) -> Result<(), StorageError> {
        // Unmount first by writing empty to UDC or clearing file
        // Writing a newline to MOUNT_DEVICE might be enough to detach the file
        let _ = fs::write(MOUNT_DEVICE, b"\n").await;

        let is_none = file_path.is_none() || file_path == Some(IMAGE_NONE);

        // CDROM mode is always read-only. Mass storage can be RO or RW.
        // For now, we follow the cdrom flag for RO.
        let ro_val = if cdrom || is_none { b"1" } else { b"0" };
        let cdrom_val = if cdrom { b"1" } else { b"0" };

        fs::write(RO_FLAG, ro_val).await?;
        fs::write(CDROM_FLAG, cdrom_val).await?;

        let inquiry_ven = "NanoKVM";
        let inquiry_prd = if cdrom { "USB CD/DVD-ROM" } else { "USB Disk" };
        let inquiry_ver = "0520";
        let inquiry_data = format!("{:<8}{:<16}{}", inquiry_ven, inquiry_prd, inquiry_ver);

        fs::write(INQUIRY_STRING, inquiry_data.as_bytes()).await?;

        let image = file_path.unwrap_or(IMAGE_NONE);
        fs::write(MOUNT_DEVICE, image.as_bytes()).await?;

        // Reset USB Gadget to ensure host re-enumerates
        Self::reset_usb_gadget().await?;

        info!("Mounted image: {} (cdrom: {})", image, cdrom);
        Ok(())
    }

    async fn reset_usb_gadget() -> Result<(), StorageError> {
        // echo "" > /sys/kernel/config/usb_gadget/g0/UDC
        fs::write(UDC_PATH, b"\n").await?;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // ls /sys/class/udc/ | head -n 1 > /sys/kernel/config/usb_gadget/g0/UDC
        let mut entries = fs::read_dir(UDC_LIST_PATH).await?;
        if let Some(entry) = entries.next_entry().await? {
            let udc_name = entry.file_name();
            fs::write(UDC_PATH, udc_name.as_encoded_bytes()).await?;
        }

        Ok(())
    }

    pub async fn get_mounted_image() -> Result<Option<String>, StorageError> {
        let content = fs::read_to_string(MOUNT_DEVICE).await?;
        let image = content.trim().to_string();
        if image == IMAGE_NONE || image.is_empty() {
            Ok(None)
        } else {
            Ok(Some(image))
        }
    }

    pub async fn get_cdrom_flag() -> Result<bool, StorageError> {
        let content = fs::read_to_string(CDROM_FLAG).await?;
        Ok(content.trim() == "1")
    }

    pub async fn delete_image(path: &str) -> Result<(), StorageError> {
        let path_buf = Path::new(path);

        // Security check: must be in /data and end with .iso or .img
        if !path.starts_with(IMAGE_DIRECTORY) {
            return Err(StorageError::InvalidFilename(
                "Not in image directory".to_string(),
            ));
        }

        let ext = path_buf
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase());

        if !matches!(ext.as_deref(), Some("iso") | Some("img")) {
            return Err(StorageError::InvalidFilename(
                "Invalid extension".to_string(),
            ));
        }

        fs::remove_file(path).await?;
        info!("Deleted image: {}", path);
        Ok(())
    }
}
