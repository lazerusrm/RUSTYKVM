#!/bin/bash
# Build and test script for NanoKVM Passkey Implementation
# Run this on a Linux machine with Rust toolchain installed

set -e

cd "$(dirname "$0")"

echo "=== NanoKVM Passkey Build Script ==="
echo ""

# Check if we're on the target platform
if [[ "$OSTYPE" == "linux-gnu"* ]]; then
    echo "Building for native Linux..."
    cargo build --package nanokvm-server
elif [[ -n "$CROSS_BUILD" ]] || [[ -n "$RISCV" ]]; then
    echo "Cross-compiling for RISCV64..."
    cargo build --package nanokvm-server --target riscv64gc-unknown-linux-gnu --release
else
    echo "Note: OpenSSL dependencies may not be available on this platform"
    echo "Run cargo check to verify code syntax:"
    cargo check --package nanokvm-server 2>&1 | head -50
fi

echo ""
echo "=== Build completed ==="
