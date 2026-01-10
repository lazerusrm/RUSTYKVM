use bytes::Bytes;
use parking_lot::Mutex;
use std::ops::Deref;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
use tracing::info;

/// Errors that can occur during KVM operations
#[derive(Error, Debug)]
pub enum KvmError {
    #[error("Image buffer is full (-3)")]
    BufferFull,
    #[error("Video Encoder Error (-2)")]
    VencError,
    #[error("Image does not exist (-1)")]
    NotExist,
    #[error("HDMI Input Resolution Error (-7)")]
    HdmiResError,
    #[error("Unsupported Resolution (-6)")]
    UnsupportedRes,
    #[error("Retrieving image, please wait (-5)")]
    Retrieving,
    #[error("Modifying resolution, please wait (-4)")]
    ModifyingRes,
    #[error("Unknown error code: {0}")]
    Unknown(i32),
}

/// A zero-copy wrapper around hardware-allocated memory.
/// Calls `free_kvmv_data` automatically when dropped.
pub struct KvmFrame {
    ptr: *mut u8,
    len: usize,
}

// Safety: The hardware buffer is safe to send between threads.
unsafe impl Send for KvmFrame {}
unsafe impl Sync for KvmFrame {}

impl AsRef<[u8]> for KvmFrame {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl KvmFrame {
    fn new(ptr: *mut u8, len: usize) -> Self {
        tracing::debug!("KvmFrame::new() - created buffer at {:p}, len={}", ptr, len);
        Self { ptr, len }
    }

    /// Returns the length of the frame in bytes
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the frame is empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Converts the frame into a `Bytes` object.
    ///
    /// NOTE: This currently uses copy_from_slice because Bytes::from_owner()
    /// has issues with hardware buffer lifetime management on this platform.
    /// The zero-copy approach caused buffer exhaustion (-3 errors) because
    /// the hardware buffers weren't being freed properly when Bytes was dropped.
    ///
    /// TODO: Investigate why from_owner() doesn't call Drop correctly and fix.
    #[inline]
    pub fn into_bytes(self) -> Bytes {
        // Copy the data - this ensures the hardware buffer is freed immediately
        // when this KvmFrame is dropped at the end of this function.
        let slice = unsafe { std::slice::from_raw_parts(self.ptr, self.len) };
        Bytes::copy_from_slice(slice)
        // self is dropped here, calling free_kvmv_data via Drop impl
    }

    /// Returns a slice reference to the frame data (for use without consuming)
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Deref for KvmFrame {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for KvmFrame {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            let mut p = self.ptr;
            tracing::debug!("KvmFrame::drop() - freeing buffer at {:p}, len={}", self.ptr, self.len);
            kvm_sys::free_kvmv_data(&mut p);
        } else {
            tracing::debug!("KvmFrame::drop() - ptr was null, skipping free");
        }
    }
}

pub struct Kvm {}

static KVM_INITIALIZED: AtomicBool = AtomicBool::new(false);
static KVM_LOCK: Mutex<()> = Mutex::new(());

impl Kvm {
    pub fn init() -> Self {
        let _guard = KVM_LOCK.lock();
        if !KVM_INITIALIZED.load(Ordering::SeqCst) {
            info!("Initializing KVM hardware...");
            // Check if library is loaded
            if kvm_sys::is_library_loaded() {
                info!("libkvm.so loaded successfully");
            } else {
                info!("WARNING: libkvm.so not loaded - video capture will not work");
            }
            kvm_sys::kvmv_init(0);
            kvm_sys::set_venc_auto_recyc(1); // Enable automatic video encoder buffer recycling
            KVM_INITIALIZED.store(true, Ordering::SeqCst);
            info!("KVM hardware initialized.");
        }
        Self {}
    }

    /// Deinitialize KVM hardware - should be called on shutdown
    pub fn deinit() {
        let _guard = KVM_LOCK.lock();
        if KVM_INITIALIZED.load(Ordering::SeqCst) {
            info!("Deinitializing KVM hardware...");
            kvm_sys::free_all_kvmv_data();
            kvm_sys::kvmv_deinit();
            KVM_INITIALIZED.store(false, Ordering::SeqCst);
            info!("KVM hardware deinitialized.");
        }
    }

    pub fn get_mjpeg(&self, width: u16, height: u16, quality: u16) -> Result<KvmFrame, KvmError> {
        self.read_img(width, height, kvm_sys::IMG_MJPEG_TYPE, quality)
    }

    pub fn get_h264(&self, width: u16, height: u16, bitrate: u16) -> Result<KvmFrame, KvmError> {
        self.read_img(width, height, kvm_sys::IMG_H264_TYPE_SPS, bitrate)
    }

    fn read_img(
        &self,
        width: u16,
        height: u16,
        img_type: u8,
        quality: u16,
    ) -> Result<KvmFrame, KvmError> {
        let mut data_ptr: *mut u8 = ptr::null_mut();
        let mut size: u32 = 0;

        let ret =
            kvm_sys::kvmv_read_img(width, height, img_type, quality, &mut data_ptr, &mut size);

        if ret < 0 {
            return Err(match ret {
                -1 => KvmError::NotExist,
                -2 => KvmError::VencError,
                -3 => KvmError::BufferFull,
                -4 => KvmError::ModifyingRes,
                -5 => KvmError::Retrieving,
                -6 => KvmError::UnsupportedRes,
                -7 => KvmError::HdmiResError,
                x => KvmError::Unknown(x),
            });
        }

        if data_ptr.is_null() || size == 0 {
            return Err(KvmError::NotExist);
        }

        Ok(KvmFrame::new(data_ptr, size as usize))
    }

    pub fn set_h264_gop(&self, gop: u8) {
        kvm_sys::set_h264_gop(gop);
    }

    pub fn set_frame_detect(&self, frame: u8) {
        kvm_sys::set_frame_detect(frame);
    }

    pub fn set_hdmi(&self, enable: bool) -> Result<(), KvmError> {
        let val = if enable { 1 } else { 0 };
        kvm_sys::kvmv_hdmi_control(val);
        Ok(())
    }
}
