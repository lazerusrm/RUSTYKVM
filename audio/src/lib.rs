use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_uint, c_void};
use thiserror::Error;
use tracing::{debug, error, info};

#[derive(Error, Debug)]
pub enum AudioError {
    #[error("Failed to open PCM device: {0}")]
    OpenFailed(i32),
    #[error("Failed to read from PCM device: {0}")]
    ReadFailed(i32),
    #[error("Invalid configuration")]
    InvalidConfig,
}

// Minimal TinyALSA FFI bindings
#[repr(C)]
struct pcm_config {
    channels: c_uint,
    rate: c_uint,
    period_size: c_uint,
    period_count: c_uint,
    format: c_uint,
    start_threshold: c_uint,
    stop_threshold: c_uint,
    silence_threshold: c_uint,
    avail_min: c_int,
}

const PCM_IN: c_uint = 0x10000000;
const PCM_FORMAT_S16_LE: c_uint = 0;

extern "C" {
    fn pcm_open(
        card: c_uint,
        device: c_uint,
        flags: c_uint,
        config: *const pcm_config,
    ) -> *mut c_void;
    fn pcm_close(pcm: *mut c_void) -> c_int;
    fn pcm_is_ready(pcm: *mut c_void) -> c_int;
    fn pcm_get_error(pcm: *mut c_void) -> *const c_char;
    fn pcm_read(pcm: *mut c_void, data: *mut c_void, count: c_uint) -> c_int;
}

pub struct AudioCapturer {
    pcm: *mut c_void,
    buffer_size: usize,
}

// Safety: The pointer is managed and only accessed via sync methods
unsafe impl Send for AudioCapturer {}

impl AudioCapturer {
    pub fn new(card: u32, device: u32) -> Result<Self, AudioError> {
        let config = pcm_config {
            channels: 2,
            rate: 48000,
            period_size: 1024,
            period_count: 4,
            format: PCM_FORMAT_S16_LE,
            start_threshold: 0,
            stop_threshold: 0,
            silence_threshold: 0,
            avail_min: 0,
        };

        unsafe {
            let pcm = pcm_open(card, device, PCM_IN, &config);
            if pcm.is_null() || pcm_is_ready(pcm) == 0 {
                let err = if !pcm.is_null() {
                    let msg = std::ffi::CStr::from_ptr(pcm_get_error(pcm)).to_string_lossy();
                    error!("PCM open error: {}", msg);
                    pcm_close(pcm);
                    -1
                } else {
                    -1
                };
                return Err(AudioError::OpenFailed(err));
            }

            info!("Audio PCM device opened: card {} device {}", card, device);
            Ok(Self {
                pcm,
                buffer_size: (1024 * 2 * 2), // period_size * channels * sizeof(s16)
            })
        }
    }

    pub fn read(&self, buffer: &mut [u8]) -> Result<usize, AudioError> {
        if buffer.len() < self.buffer_size {
            return Err(AudioError::InvalidConfig);
        }

        unsafe {
            let res = pcm_read(
                self.pcm,
                buffer.as_mut_ptr() as *mut c_void,
                self.buffer_size as c_uint,
            );
            if res < 0 {
                return Err(AudioError::ReadFailed(res));
            }
            Ok(self.buffer_size)
        }
    }
}

impl Drop for AudioCapturer {
    fn drop(&mut self) {
        unsafe {
            if !self.pcm.is_null() {
                pcm_close(self.pcm);
                info!("Audio PCM device closed");
            }
        }
    }
}
