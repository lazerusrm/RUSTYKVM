# NanoKVM Rust Port - Build Guide

## Target Hardware
- **CPU**: T-Head C906 (RISC-V 64-bit)
- **Architecture**: rv64imafdcv (RISC-V 64-bit with vector extensions)
- **OS**: Linux (Buildroot-based)

## Cross-Compilation Setup

### 1. Install Rust Target
```bash
rustup target add riscv64gc-unknown-linux-gnu
```

### 2. Install Cross-Compiler (in Docker)
```bash
apt-get install -y gcc-riscv64-linux-gnu g++-riscv64-linux-gnu
```

### 3. Build Command
```bash
# In Docker container:
cd /home/build/NanoKVM/nanokvm-rs

# Set OpenSSL sysroot for cross-compilation
export OPENSSL_LIB_DIR=/usr/riscv64-linux-gnu/lib
export OPENSSL_INCLUDE_DIR=/usr/riscv64-linux-gnu/include

# Build
cargo build --release --target riscv64gc-unknown-linux-gnu
```

### 4. Output Location
```
target/riscv64gc-unknown-linux-gnu/release/nanokvm-server
```

## Build in Docker
```bash
# From project root:
make app

# Or manually:
docker run -it --rm \
  -v $(pwd):/home/build/NanoKVM \
  -e OPENSSL_LIB_DIR=/usr/riscv64-linux-gnu/lib \
  -e OPENSSL_INCLUDE_DIR=/usr/riscv64-linux-gnu/include \
  nanokvm-builder \
  bash -c "cd /home/build/NanoKVM/nanokvm-rs && cargo build --release --target riscv64gc-unknown-linux-gnu"
```

## Key Configuration Files
- `.cargo/config.toml` - Cross-compilation toolchain settings
- `server/Cargo.toml` - Platform-specific dependencies
- `kvm-sys/build.rs` - Hardware library linking

## QA/QC Fixes Applied
| Issue | Status |
|-------|--------|
| Duplicate serde dependency | ✅ Fixed |
| `set_frame_detact` typo | ✅ Fixed |
| Missing `SCRIPT_DIRECTORY` | ✅ Fixed |
| `inquiryVen` typo | ✅ Fixed |
| JWT secret externalization | ✅ Fixed |
| Input sanitization (network.rs) | ✅ Fixed |
| Input validation (vm.rs) | ✅ Fixed |
| `compare` → `verify` panic | ✅ Fixed |
| Platform-specific code guards | ✅ Added |
| Blocking PTY reads | ✅ Fixed |
| RISC-V cross-compilation | ✅ Configured |
