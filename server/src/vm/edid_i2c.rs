//! Direct I2C EDID read — register sequences ported from `nanokvm_update_edid.c`.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::thread;
use std::time::Duration;

const I2C_DEVICE: &str = "/dev/i2c-4";
const I2C_ADDRESS: u16 = 0x2b;
const VERSION_PATH: &str = "/etc/kvm/hdmi_version";

const LT6911_REG_OFFSET: u8 = 0xFF;
const LT6911_SYS_OFFSET: u8 = 0x80;
const LT6911_SYS2_OFFSET: u8 = 0x90;
const LT6911_SYS3_OFFSET: u8 = 0x81;
const LT6911_SYS4_OFFSET: u8 = 0xA0;
const LT6911UXC_WR_SIZE: usize = 32;
const LT6911C_WR_SIZE: usize = 16;
const EDID_BUFFER_SIZE: usize = 256;

const I2C_SLAVE: libc::c_ulong = 0x0703;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChipVersion {
    Lt6911Uxc,
    Lt6911C,
}

struct Lt6911I2c {
    client: std::fs::File,
    old_offset: u8,
}

impl Lt6911I2c {
    fn open() -> Result<Self, String> {
        let client = OpenOptions::new()
            .read(true)
            .write(true)
            .open(I2C_DEVICE)
            .map_err(|e| format!("Failed to open {}: {}", I2C_DEVICE, e))?;

        let ret = unsafe {
            libc::ioctl(
                client.as_raw_fd(),
                I2C_SLAVE as _,
                I2C_ADDRESS as libc::c_ulong,
            )
        };
        if ret < 0 {
            return Err(format!(
                "Failed to set I2C slave address 0x{:02X}: {}",
                I2C_ADDRESS,
                std::io::Error::last_os_error()
            ));
        }

        Ok(Self {
            client,
            old_offset: 0xff,
        })
    }

    fn i2c_write_byte(&mut self, offset: u8, reg: u8, data: u8) -> Result<(), String> {
        if offset != self.old_offset {
            self.old_offset = offset;
            self.client
                .write_all(&[LT6911_REG_OFFSET, offset])
                .map_err(|e| format!("I2C offset write failed: {}", e))?;
        }
        self.client
            .write_all(&[reg, data])
            .map_err(|e| format!("I2C write failed: {}", e))
    }

    fn i2c_write_bytes(&mut self, offset: u8, reg: u8, data: &[u8]) -> Result<(), String> {
        if data.is_empty() {
            return Err("I2C write length must be > 0".to_string());
        }
        if offset != self.old_offset {
            self.old_offset = offset;
            self.client
                .write_all(&[LT6911_REG_OFFSET, offset])
                .map_err(|e| format!("I2C offset write failed: {}", e))?;
        }
        let mut buf = Vec::with_capacity(1 + data.len());
        buf.push(reg);
        buf.extend_from_slice(data);
        self.client
            .write_all(&buf)
            .map_err(|e| format!("I2C write failed: {}", e))
    }

    fn i2c_read_byte(&mut self, offset: u8, reg: u8) -> Result<u8, String> {
        let mut data = [0u8];
        self.i2c_read_bytes(offset, reg, &mut data)?;
        Ok(data[0])
    }

    fn i2c_read_bytes(&mut self, offset: u8, reg: u8, data: &mut [u8]) -> Result<(), String> {
        if data.is_empty() {
            return Err("I2C read length must be > 0".to_string());
        }
        if offset != self.old_offset {
            self.old_offset = offset;
            self.client
                .write_all(&[LT6911_REG_OFFSET, offset])
                .map_err(|e| format!("I2C offset write failed: {}", e))?;
        }
        self.client
            .write_all(&[reg])
            .map_err(|e| format!("I2C register write failed: {}", e))?;
        self.client
            .read_exact(data)
            .map_err(|e| format!("I2C read failed: {}", e))
    }

    fn lt6911_enable(&mut self) -> Result<(), String> {
        self.i2c_write_byte(LT6911_SYS_OFFSET, 0xEE, 0x01)
    }

    #[allow(dead_code)]
    fn lt6911_disable(&mut self) -> Result<(), String> {
        self.i2c_write_byte(LT6911_SYS_OFFSET, 0xEE, 0x00)
    }

    fn lt6911uxc_edid_read(&mut self, edid_data: &mut [u8]) -> Result<(), String> {
        let wr_count = edid_data.len() / LT6911UXC_WR_SIZE;
        self.i2c_write_byte(LT6911_SYS_OFFSET, 0xFF, 0x80)?;
        self.lt6911_enable()?;
        self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0x84)?;
        self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0x80)?;

        for i in 0..wr_count {
            let base = LT6911UXC_WR_SIZE * i;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5E, 0x5F)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0xA0)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0x80)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5B, 0x01)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5C, 0x80)?;
            self.i2c_write_byte(
                LT6911_SYS_OFFSET,
                0x5D,
                0x00u8.wrapping_add((LT6911UXC_WR_SIZE * i) as u8),
            )?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0x90)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0x80)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x58, 0x21)?;
            self.i2c_read_bytes(
                LT6911_SYS_OFFSET,
                0x5F,
                &mut edid_data[base..base + LT6911UXC_WR_SIZE],
            )?;
        }
        Ok(())
    }

    fn lt6911c_edid_read(&mut self, edid_data: &mut [u8]) -> Result<(), String> {
        let wr_count = edid_data.len() / LT6911C_WR_SIZE;
        self.i2c_write_byte(LT6911_SYS_OFFSET, 0xFF, 0x80)?;
        self.lt6911_enable()?;
        self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0x86)?;
        self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0x82)?;

        for i in 0..wr_count {
            let base = LT6911C_WR_SIZE * i;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5E, 0x6F)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0xA2)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0x82)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5B, 0x01)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5C, 0x80)?;
            self.i2c_write_byte(
                LT6911_SYS_OFFSET,
                0x5D,
                0x00u8.wrapping_add((LT6911C_WR_SIZE * i) as u8),
            )?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0x92)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x5A, 0x82)?;
            self.i2c_write_byte(LT6911_SYS_OFFSET, 0x58, 0x01)?;
            self.i2c_read_bytes(
                LT6911_SYS_OFFSET,
                0x5F,
                &mut edid_data[base..base + LT6911C_WR_SIZE],
            )?;
        }
        Ok(())
    }
}

fn detect_chip_version() -> Result<ChipVersion, String> {
    let raw = std::fs::read_to_string(VERSION_PATH)
        .map_err(|e| format!("Failed to read {}: {}", VERSION_PATH, e))?;
    let version = raw.trim();
    match version {
        "c" => Ok(ChipVersion::Lt6911C),
        "ux" => Ok(ChipVersion::Lt6911Uxc),
        "ue" => Err("UE chip version does not support EDID updates".to_string()),
        other => Err(format!(
            "Unknown chip version in {}: {}",
            VERSION_PATH, other
        )),
    }
}

fn read_edid_blocking() -> Result<Vec<u8>, String> {
    let chip = detect_chip_version()?;
    let mut i2c = Lt6911I2c::open()?;
    let mut edid = vec![0u8; EDID_BUFFER_SIZE];

    match chip {
        ChipVersion::Lt6911Uxc => i2c.lt6911uxc_edid_read(&mut edid)?,
        ChipVersion::Lt6911C => i2c.lt6911c_edid_read(&mut edid)?,
    }

    Ok(edid)
}

/// Read current EDID from the LT6911 receiver over I2C (Linux device only).
pub async fn read_edid_from_hardware() -> Result<Vec<u8>, String> {
    tokio::task::spawn_blocking(|| {
        // Small settle delay mirrors post-write behavior in the C tool.
        thread::sleep(Duration::from_millis(50));
        read_edid_blocking()
    })
    .await
    .map_err(|e| format!("EDID read task failed: {}", e))?
}
