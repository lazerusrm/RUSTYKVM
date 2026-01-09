use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    // Only use stubs if KVM_STUB is explicitly set
    // CI builds for RISC-V should link dynamically against libkvm on the device
    let use_stub = env::var("KVM_STUB").is_ok();

    if use_stub {
        // Create stub library for CI builds
        let stub_c = out_dir.join("kvm_stub.c");
        fs::write(
            &stub_c,
            r#"
// Stub implementations for CI builds
// These functions exist on the real NanoKVM hardware

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
    return -1; // IMG_NOT_EXIST
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

        // Compile the stub
        cc::Build::new().file(&stub_c).compile("kvm");

        println!("cargo:warning=Building with stub libkvm (CI mode)");
    } else {
        // Link against the real library
        println!("cargo:rustc-link-lib=kvm");

        // For cross-compilation, search in standard locations
        if target.contains("riscv64") {
            // RISC-V NanoKVM target
            println!("cargo:rustc-link-search=native=/usr/riscv64-linux-gnu/lib");
            println!("cargo:rustc-link-search=native=/lib/riscv64-linux-gnu");
            println!("cargo:rustc-link-search=native=/usr/lib/riscv64-linux-gnu");
        } else if target.contains("linux") {
            // Linux native or other cross-compile
            println!("cargo:rustc-link-search=native=/usr/lib");
            println!("cargo:rustc-link-search=native=/lib");
        }

        // Allow override via environment variable
        if let Ok(lib_path) = env::var("KVM_LIB_PATH") {
            println!("cargo:rustc-link-search=native={}", lib_path);
        }
    }
}
