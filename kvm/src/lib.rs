use bytes::Bytes;
use parking_lot::Mutex;
use std::ops::Deref;
use std::ptr;
use std::sync::atomic::{AtomicU8, Ordering};
use thiserror::Error;
use tracing::{info, warn};

/// Errors that can occur during KVM operations
#[derive(Error, Debug, Clone)]
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
    #[error("KVM hardware not initialized - no HDMI signal detected")]
    NotInitialized,
    #[error("libkvm.so library not loaded")]
    LibraryNotLoaded,
}

/// H.264 frame types returned by the hardware encoder
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H264FrameType {
    Sps = 1,
    Pps = 2,
    IFrame = 3,
    PFrame = 4,
    Unknown = 0,
}

impl From<i32> for H264FrameType {
    fn from(val: i32) -> Self {
        match val {
            1 => H264FrameType::Sps,
            2 => H264FrameType::Pps,
            3 => H264FrameType::IFrame,
            4 => H264FrameType::PFrame,
            _ => H264FrameType::Unknown,
        }
    }
}

/// A zero-copy wrapper around hardware-allocated memory.
/// Calls `free_kvmv_data` automatically when dropped.
pub struct KvmFrame {
    ptr: *mut u8,
    len: usize,
    /// For H.264 frames, indicates the frame type (I-frame, P-frame, etc.)
    pub frame_type: H264FrameType,
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
    fn new(ptr: *mut u8, len: usize, frame_type: H264FrameType) -> Self {
        tracing::debug!(
            "KvmFrame::new() - created buffer at {:p}, len={}, type={:?}",
            ptr,
            len,
            frame_type
        );
        Self {
            ptr,
            len,
            frame_type,
        }
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
    /// NOTE: This uses copy_from_slice to ensure the hardware buffer is freed
    /// immediately. The zero-copy approach using Bytes::from_owner() caused
    /// buffer exhaustion because hardware buffers weren't freed when Bytes
    /// was cloned across broadcast channels.
    #[inline]
    pub fn into_bytes(self) -> Bytes {
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
            tracing::debug!(
                "KvmFrame::drop() - freeing buffer at {:p}, len={}",
                self.ptr,
                self.len
            );
            kvm_sys::free_kvmv_data(&mut p);
        } else {
            tracing::debug!("KvmFrame::drop() - ptr was null, skipping free");
        }
    }
}

/// KVM initialization state
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum InitState {
    NotStarted = 0,
    Initializing = 1,
    Ready = 2,
    Failed = 3,
}

impl From<u8> for InitState {
    fn from(v: u8) -> Self {
        match v {
            0 => InitState::NotStarted,
            1 => InitState::Initializing,
            2 => InitState::Ready,
            3 => InitState::Failed,
            _ => InitState::NotStarted,
        }
    }
}

/// Encoder mode - tracks which encoder type is currently active
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EncoderMode {
    None = 0,
    Mjpeg = 1,
    H264 = 2,
}

impl From<u8> for EncoderMode {
    fn from(v: u8) -> Self {
        match v {
            1 => EncoderMode::Mjpeg,
            2 => EncoderMode::H264,
            _ => EncoderMode::None,
        }
    }
}

pub struct Kvm {
    /// Track consecutive initialization failures for backoff
    init_failures: AtomicU8,
}

static KVM_STATE: AtomicU8 = AtomicU8::new(0); // InitState::NotStarted
static KVM_LOCK: Mutex<()> = Mutex::new(());
static ENCODER_MODE: AtomicU8 = AtomicU8::new(0); // EncoderMode::None

impl Kvm {
    /// Create a new KVM instance without initializing hardware.
    /// Hardware initialization is deferred until first frame request.
    /// This prevents crashes when HDMI is not connected at startup.
    pub fn new() -> Self {
        info!("KVM instance created (hardware init deferred until first use)");
        Self {
            init_failures: AtomicU8::new(0),
        }
    }

    /// Legacy init() for compatibility - now just calls new()
    pub fn init() -> Self {
        Self::new()
    }

    /// Check if KVM hardware is initialized and ready
    pub fn is_ready(&self) -> bool {
        InitState::from(KVM_STATE.load(Ordering::SeqCst)) == InitState::Ready
    }

    /// Attempt to initialize KVM hardware.
    /// Returns Ok(true) if newly initialized, Ok(false) if already ready,
    /// or Err if initialization failed.
    fn ensure_initialized(&self) -> Result<bool, KvmError> {
        let state = InitState::from(KVM_STATE.load(Ordering::SeqCst));

        match state {
            InitState::Ready => return Ok(false),
            InitState::Initializing => {
                // Another thread is initializing, wait a bit and check again
                std::thread::sleep(std::time::Duration::from_millis(50));
                return if self.is_ready() {
                    Ok(false)
                } else {
                    Err(KvmError::NotInitialized)
                };
            }
            InitState::Failed => {
                // Check if we should retry (exponential backoff)
                let failures = self.init_failures.load(Ordering::SeqCst);
                if failures >= 5 {
                    // After 5 failures, only retry every ~30 seconds
                    // (caller should implement their own retry logic)
                    return Err(KvmError::NotInitialized);
                }
            }
            InitState::NotStarted => {}
        }

        // Try to acquire initialization lock
        let _guard = KVM_LOCK.lock();

        // Double-check state after acquiring lock
        let state = InitState::from(KVM_STATE.load(Ordering::SeqCst));
        if state == InitState::Ready {
            return Ok(false);
        }

        // Check if library is loaded
        if !kvm_sys::is_library_loaded() {
            warn!("libkvm.so not loaded - video capture will not work");
            KVM_STATE.store(InitState::Failed as u8, Ordering::SeqCst);
            return Err(KvmError::LibraryNotLoaded);
        }

        // Mark as initializing
        KVM_STATE.store(InitState::Initializing as u8, Ordering::SeqCst);
        info!("Initializing KVM hardware...");

        // Initialize the hardware
        // Note: This can potentially crash if HDMI hardware is in bad state.
        // We wrap in catch_unwind but C crashes (segfaults) cannot be caught.
        kvm_sys::kvmv_init(0);
        kvm_sys::set_venc_auto_recyc(1);

        // Enable frame detection by default (60 = check every 60 frames for changes)
        kvm_sys::set_frame_detect(60);

        // Give the MMF (multimedia framework) time to fully initialize
        // The hardware continues initializing after kvmv_init returns
        // Use longer delay (2s) to ensure all VI channels are added and ISP is ready
        std::thread::sleep(std::time::Duration::from_millis(2000));

        // If we got here, initialization succeeded
        KVM_STATE.store(InitState::Ready as u8, Ordering::SeqCst);
        self.init_failures.store(0, Ordering::SeqCst);
        info!("KVM hardware initialized successfully");

        Ok(true)
    }

    /// Reset initialization state to allow retry.
    /// Call this after HDMI is reconnected to re-attempt initialization.
    pub fn reset_init_state(&self) {
        let _guard = KVM_LOCK.lock();
        let state = InitState::from(KVM_STATE.load(Ordering::SeqCst));
        if state == InitState::Failed {
            info!("Resetting KVM init state for retry");
            KVM_STATE.store(InitState::NotStarted as u8, Ordering::SeqCst);
            self.init_failures.store(0, Ordering::SeqCst);
        }
    }

    /// Deinitialize KVM hardware - should be called on shutdown
    pub fn deinit() {
        let _guard = KVM_LOCK.lock();
        let state = InitState::from(KVM_STATE.load(Ordering::SeqCst));
        if state == InitState::Ready {
            info!("Deinitializing KVM hardware...");
            kvm_sys::free_all_kvmv_data();
            kvm_sys::kvmv_deinit();
            KVM_STATE.store(InitState::NotStarted as u8, Ordering::SeqCst);
            ENCODER_MODE.store(EncoderMode::None as u8, Ordering::SeqCst);
            info!("KVM hardware deinitialized.");
        }
    }

    /// Get the current encoder mode
    pub fn get_encoder_mode(&self) -> EncoderMode {
        EncoderMode::from(ENCODER_MODE.load(Ordering::SeqCst))
    }

    /// Switch encoder mode with proper cleanup to prevent ION memory exhaustion.
    /// This MUST be called before switching between MJPEG and H.264 to release
    /// the hardware VB pools allocated by the previous encoder mode.
    ///
    /// IMPORTANT: Caller must hold the KVM_LOCK.
    fn switch_encoder_mode_locked(&self, new_mode: EncoderMode) {
        let current = EncoderMode::from(ENCODER_MODE.load(Ordering::SeqCst));
        if current == new_mode {
            return; // No switch needed
        }

        if current != EncoderMode::None {
            // Mode is changing
            // NOTE: Do NOT call free_all_kvmv_data() here - the libkvm.so library
            // handles encoder mode switching internally and will reinitialize MMF.
            // Calling free while MMF is reinitializing causes SIGSEGV.
            info!(
                "Switching encoder mode from {:?} to {:?}",
                current, new_mode
            );

            // Give time for any pending frame operations to complete
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        ENCODER_MODE.store(new_mode as u8, Ordering::SeqCst);
    }

    pub fn get_mjpeg(&self, width: u16, height: u16, quality: u16) -> Result<KvmFrame, KvmError> {
        // Ensure hardware is initialized before reading
        if let Err(e) = self.ensure_initialized() {
            return Err(e);
        }

        // Lock to prevent concurrent encoder access
        let _guard = KVM_LOCK.lock();

        // Handle encoder mode switch with proper cleanup
        self.switch_encoder_mode_locked(EncoderMode::Mjpeg);
        self.read_img(width, height, kvm_sys::IMG_MJPEG_TYPE, quality)
    }

    pub fn get_h264(&self, width: u16, height: u16, bitrate: u16) -> Result<KvmFrame, KvmError> {
        // Ensure hardware is initialized before reading
        if let Err(e) = self.ensure_initialized() {
            return Err(e);
        }

        // Lock to prevent concurrent encoder access
        let _guard = KVM_LOCK.lock();

        // Handle encoder mode switch with proper cleanup
        self.switch_encoder_mode_locked(EncoderMode::H264);
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
            // Track failures for init retry backoff
            if ret == -7 || ret == -2 {
                // HDMI or encoder error - might need reinit
                let failures = self.init_failures.fetch_add(1, Ordering::SeqCst);
                if failures > 10 {
                    warn!("Many consecutive video errors, may need HDMI reconnect");
                }
            }

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

        // Reset failure counter on success
        self.init_failures.store(0, Ordering::SeqCst);

        if data_ptr.is_null() || size == 0 {
            return Err(KvmError::NotExist);
        }

        let frame_type = H264FrameType::from(ret);
        Ok(KvmFrame::new(data_ptr, size as usize, frame_type))
    }

    pub fn set_h264_gop(&self, gop: u8) {
        if self.is_ready() {
            kvm_sys::set_h264_gop(gop);
        }
    }

    pub fn set_frame_detect(&self, frame: u8) {
        if self.is_ready() {
            kvm_sys::set_frame_detect(frame);
        }
    }

    pub fn set_hdmi(&self, enable: bool) -> Result<(), KvmError> {
        // HDMI control should work even before full init
        if !kvm_sys::is_library_loaded() {
            return Err(KvmError::LibraryNotLoaded);
        }

        let val = if enable { 1 } else { 0 };
        kvm_sys::kvmv_hdmi_control(val);

        // If enabling HDMI, reset init state to allow retry
        if enable {
            self.reset_init_state();
        }

        Ok(())
    }
}

impl Default for Kvm {
    fn default() -> Self {
        Self::new()
    }
}
