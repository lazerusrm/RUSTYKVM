use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // For cross-compilation (CI builds), we create a stub library that the linker
    // can use at compile time. The real libkvm.so on the device will be used at runtime.
    // For native builds on the device, we link against the real library.
    let is_cross_compile = target.contains("riscv64") && !cfg!(target_arch = "riscv64");
    let use_stub = env::var("KVM_STUB").is_ok() || is_cross_compile;

    if use_stub {
        // Create stub library for cross-compilation
        // These stubs provide symbols for the linker but won't be used at runtime
        // (the real libkvm.so on the device will be loaded instead)
        let stub_c = out_dir.join("kvm_stub.c");
        fs::write(
            &stub_c,
            r#"
// Stub implementations for cross-compilation
// At runtime on the device, the real libkvm.so will be used via LD_LIBRARY_PATH

void kvmv_init(unsigned char debug_info_en) { (void)debug_info_en; }
void set_venc_auto_recyc(unsigned char enable) { (void)enable; }

int kvmv_read_img(
    unsigned short width,
    unsigned short height,
    unsigned char type,
    unsigned short qlty,
    unsigned char** pp_kvm_data,
    unsigned int* p_kvmv_data_size
) {
    (void)width; (void)height; (void)type; (void)qlty;
    *pp_kvm_data = (unsigned char*)0;
    *p_kvmv_data_size = 0;
    return -1;
}

int free_kvmv_data(unsigned char** pp_kvm_data) {
    (void)pp_kvm_data;
    return 0;
}

void free_all_kvmv_data(void) {}

void set_h264_gop(unsigned char gop) { (void)gop; }
void set_frame_detect(unsigned char frame_detect) { (void)frame_detect; }

void kvmv_deinit(void) {}
unsigned char kvmv_hdmi_control(unsigned char en) { (void)en; return 0; }
"#,
        )
        .expect("Failed to write stub C file");

        // Compile the stub as a static library
        cc::Build::new().file(&stub_c).compile("kvm");

        if is_cross_compile {
            println!(
                "cargo:warning=Cross-compiling: using stub libkvm (real library needed on device)"
            );
        } else {
            println!("cargo:warning=Building with stub libkvm (KVM_STUB mode)");
        }
    } else {
        // Native build on device - link against the real library dynamically
        println!("cargo:rustc-link-lib=dylib=kvm");
        println!("cargo:rustc-link-search=native=/kvmapp/dl_lib");
        println!("cargo:rustc-link-search=native=/usr/lib");
        println!("cargo:rustc-link-search=native=/lib");

        // Allow override via environment variable
        if let Ok(lib_path) = env::var("KVM_LIB_PATH") {
            println!("cargo:rustc-link-search=native={}", lib_path);
        }
    }
}
