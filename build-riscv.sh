#!/bin/bash
# Cross-compilation script for NanoKVM Rust code
# Run this inside the Docker container

set -e

# Install Rust RISC-V target if not already installed
rustup target add riscv64gc-unknown-linux-gnu

# Install cross-compiler toolchain (if not in Docker)
if ! command -v riscv64-unknown-linux-gnu-gcc &> /dev/null; then
    apt-get update && apt-get install -y gcc-riscv64-linux-gnu g++-riscv64-linux-gnu
fi

# Build with cargo
cd /home/build/NanoKVM/nanokvm-rs

# Clean previous builds
cargo clean

# Build for RISC-V
echo "Building for riscv64gc-unknown-linux-gnu..."
cargo build --release --target riscv64gc-unknown-linux-gnu

# Output location
echo "Build complete!"
echo "Binary location: nanokvm-rs/target/riscv64gc-unknown-linux-gnu/release/nanokvm-server"
