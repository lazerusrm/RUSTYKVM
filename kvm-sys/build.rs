fn main() {
    println!("cargo:rustc-link-lib=kvm");

    let target = std::env::var("TARGET").unwrap_or_default();

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
    if let Ok(lib_path) = std::env::var("KVM_LIB_PATH") {
        println!("cargo:rustc-link-search=native={}", lib_path);
    }
}
