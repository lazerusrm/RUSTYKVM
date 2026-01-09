use libc::c_int;
use libloading::{Library, Symbol};
use once_cell::sync::OnceCell;
use std::sync::Mutex;

// Constants from kvm_vision.h
pub const IMG_BUFFER_FULL: c_int = -3;
pub const IMG_VENC_ERROR: c_int = -2;
pub const IMG_NOT_EXIST: c_int = -1;

pub const IMG_MJPEG_TYPE: u8 = 0;
pub const IMG_H264_TYPE_SPS: u8 = 1;
pub const IMG_H264_TYPE_PPS: u8 = 2;
pub const IMG_H264_TYPE_IF: u8 = 3;
pub const IMG_H264_TYPE_PF: u8 = 4;

// Library search paths for libkvm.so
const LIB_PATHS: &[&str] = &[
    "./dl_lib/libkvm.so",
    "/kvmapp/server/dl_lib/libkvm.so",
    "/tmp/server/dl_lib/libkvm.so",
    "libkvm.so",
];

// Global library handle
static KVM_LIB: OnceCell<Mutex<Option<Library>>> = OnceCell::new();

// Function type definitions
type KvmvInitFn = unsafe extern "C" fn(u8);
type SetVencAutoRecycFn = unsafe extern "C" fn(u8);
type KvmvReadImgFn = unsafe extern "C" fn(u16, u16, u8, u16, *mut *mut u8, *mut u32) -> c_int;
type FreeKvmvDataFn = unsafe extern "C" fn(*mut *mut u8) -> c_int;
type FreeAllKvmvDataFn = unsafe extern "C" fn();
type SetH264GopFn = unsafe extern "C" fn(u8);
type SetFrameDetectFn = unsafe extern "C" fn(u8);
type KvmvDeinitFn = unsafe extern "C" fn();
type KvmvHdmiControlFn = unsafe extern "C" fn(u8) -> u8;

fn get_library() -> &'static Mutex<Option<Library>> {
    KVM_LIB.get_or_init(|| {
        // Try loading from various paths
        for path in LIB_PATHS {
            if let Ok(lib) = unsafe { Library::new(path) } {
                eprintln!("[kvm-sys] Loaded libkvm.so from: {}", path);
                return Mutex::new(Some(lib));
            }
        }
        eprintln!("[kvm-sys] WARNING: Could not load libkvm.so - video capture will not work");
        Mutex::new(None)
    })
}

macro_rules! call_kvm_fn {
    ($name:expr, $fn_type:ty, $($arg:expr),*) => {{
        let lib_guard = get_library().lock().unwrap();
        if let Some(ref lib) = *lib_guard {
            unsafe {
                if let Ok(func) = lib.get::<Symbol<$fn_type>>($name.as_bytes()) {
                    return func($($arg),*);
                }
            }
        }
    }};
}

macro_rules! call_kvm_fn_void {
    ($name:expr, $fn_type:ty, $($arg:expr),*) => {{
        let lib_guard = get_library().lock().unwrap();
        if let Some(ref lib) = *lib_guard {
            unsafe {
                if let Ok(func) = lib.get::<Symbol<$fn_type>>($name.as_bytes()) {
                    func($($arg),*);
                    return;
                }
            }
        }
    }};
}

/// Initialize KVM video hardware
pub fn kvmv_init(debug_info_en: u8) {
    call_kvm_fn_void!("kvmv_init", KvmvInitFn, debug_info_en);
    eprintln!("[kvm-sys] kvmv_init: library not loaded");
}

/// Enable/disable automatic video encoder buffer recycling
pub fn set_venc_auto_recyc(enable: u8) {
    call_kvm_fn_void!("set_venc_auto_recyc", SetVencAutoRecycFn, enable);
}

/// Read a video frame from the capture hardware
///
/// # Arguments
/// * `width` - Requested frame width
/// * `height` - Requested frame height
/// * `img_type` - Image type (MJPEG or H264)
/// * `quality` - JPEG quality (1-100) or H264 bitrate
/// * `pp_kvm_data` - Output pointer to frame data
/// * `p_kvmv_data_size` - Output frame size in bytes
///
/// # Returns
/// * 0 on success
/// * IMG_NOT_EXIST (-1) if no frame available
/// * IMG_VENC_ERROR (-2) on encoder error
/// * IMG_BUFFER_FULL (-3) if buffer is full
pub fn kvmv_read_img(
    width: u16,
    height: u16,
    img_type: u8,
    quality: u16,
    pp_kvm_data: *mut *mut u8,
    p_kvmv_data_size: *mut u32,
) -> c_int {
    let lib_guard = get_library().lock().unwrap();
    if let Some(ref lib) = *lib_guard {
        unsafe {
            if let Ok(func) = lib.get::<Symbol<KvmvReadImgFn>>(b"kvmv_read_img") {
                let ret = func(
                    width,
                    height,
                    img_type,
                    quality,
                    pp_kvm_data,
                    p_kvmv_data_size,
                );
                // Debug: log first few calls and any errors
                static DEBUG_COUNT: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                let count = DEBUG_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if count < 10 || ret < 0 {
                    let size = if p_kvmv_data_size.is_null() {
                        0
                    } else {
                        *p_kvmv_data_size
                    };
                    let ptr_val = if pp_kvm_data.is_null() {
                        0
                    } else {
                        *pp_kvm_data as usize
                    };
                    eprintln!(
                        "[kvm-sys] kvmv_read_img({}x{}, type={}, q={}) -> ret={}, size={}, ptr={:#x}",
                        width, height, img_type, quality, ret, size, ptr_val
                    );
                }
                return ret;
            } else {
                eprintln!("[kvm-sys] kvmv_read_img: symbol not found");
            }
        }
    } else {
        eprintln!("[kvm-sys] kvmv_read_img: library not loaded");
    }
    IMG_NOT_EXIST
}

/// Free a single video frame buffer
pub fn free_kvmv_data(pp_kvm_data: *mut *mut u8) -> c_int {
    call_kvm_fn!("free_kvmv_data", FreeKvmvDataFn, pp_kvm_data);
    0
}

/// Free all video frame buffers
pub fn free_all_kvmv_data() {
    call_kvm_fn_void!("free_all_kvmv_data", FreeAllKvmvDataFn,);
}

/// Set H264 GOP (Group of Pictures) size
pub fn set_h264_gop(gop: u8) {
    call_kvm_fn_void!("set_h264_gop", SetH264GopFn, gop);
}

/// Enable/disable frame detection
/// Note: The C library has a typo - it's "detact" not "detect"
pub fn set_frame_detect(frame_detect: u8) {
    call_kvm_fn_void!("set_frame_detact", SetFrameDetectFn, frame_detect);
}

/// Deinitialize KVM video hardware
pub fn kvmv_deinit() {
    call_kvm_fn_void!("kvmv_deinit", KvmvDeinitFn,);
}

/// Control HDMI output
pub fn kvmv_hdmi_control(en: u8) -> u8 {
    call_kvm_fn!("kvmv_hdmi_control", KvmvHdmiControlFn, en);
    0
}

/// Check if libkvm.so is loaded
pub fn is_library_loaded() -> bool {
    let lib_guard = get_library().lock().unwrap();
    lib_guard.is_some()
}
