use libc::c_int;

// Constants from kvm_vision.h
pub const IMG_BUFFER_FULL: c_int = -3;
pub const IMG_VENC_ERROR: c_int = -2;
pub const IMG_NOT_EXIST: c_int = -1;

pub const IMG_MJPEG_TYPE: u8 = 0;
pub const IMG_H264_TYPE_SPS: u8 = 1;
pub const IMG_H264_TYPE_PPS: u8 = 2;
pub const IMG_H264_TYPE_IF: u8 = 3;
pub const IMG_H264_TYPE_PF: u8 = 4;

#[link(name = "kvm")]
extern "C" {
    pub fn kvmv_init(_debug_info_en: u8);
    pub fn set_venc_auto_recyc(_enable: u8);

    pub fn kvmv_read_img(
        _width: u16,
        _height: u16,
        _type: u8,
        _qlty: u16,
        _pp_kvm_data: *mut *mut u8,
        _p_kvmv_data_size: *mut u32,
    ) -> c_int;

    pub fn free_kvmv_data(_pp_kvm_data: *mut *mut u8) -> c_int;
    pub fn free_all_kvmv_data();

    pub fn set_h264_gop(_gop: u8);
    pub fn set_frame_detect(_frame_detect: u8);

    pub fn kvmv_deinit();
    pub fn kvmv_hdmi_control(_en: u8) -> u8;
}
