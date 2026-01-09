#!/bin/bash
# Build and package complete NanoKVM-RS firmware
# Prerequisites: Run bootstrap-deploy.sh first to populate deploy/ directory

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Check if deploy directory exists
if [ ! -d "deploy/kvmapp/server/dl_lib" ]; then
    echo "ERROR: deploy/ directory not properly initialized"
    echo "Please run ./bootstrap-deploy.sh first"
    exit 1
fi

# Determine target
TARGET="${TARGET:-riscv64gc-unknown-linux-musl}"
RELEASE_DIR="target/$TARGET/release"

echo "==> Building NanoKVM-RS for $TARGET..."
echo ""

# Build options
BUILD_OPTS="--release --target $TARGET"

# Check if we're cross-compiling
if [ "$TARGET" = "riscv64gc-unknown-linux-musl" ]; then
    # Cross-compilation mode
    echo "Cross-compiling for RISC-V..."

    # Check for toolchain
    if ! command -v riscv64-unknown-linux-musl-gcc &> /dev/null; then
        echo "WARNING: riscv64-unknown-linux-musl-gcc not found"
        echo "Using stub library mode for CI builds"
        export KVM_STUB=1
    fi

    cargo build $BUILD_OPTS
else
    # Native build
    echo "Building natively..."
    cargo build $BUILD_OPTS
fi

# Check if binary was built
BINARY_NAME="nanokvm-server"
if [ ! -f "$RELEASE_DIR/$BINARY_NAME" ]; then
    echo "ERROR: Binary not found at $RELEASE_DIR/$BINARY_NAME"
    echo "Build may have failed"
    exit 1
fi

echo ""
echo "==> Copying binary to deploy directory..."
cp "$RELEASE_DIR/$BINARY_NAME" deploy/kvmapp/server/nanokvm-rs-server
chmod +x deploy/kvmapp/server/nanokvm-rs-server

# Try to set rpath if patchelf is available (Linux only)
if command -v patchelf &> /dev/null; then
    echo "==> Setting runtime library path with patchelf..."
    patchelf --set-rpath '$ORIGIN/dl_lib' deploy/kvmapp/server/nanokvm-rs-server || true
fi

echo ""
echo "==> Creating firmware package..."
cd deploy

# Create ZIP with proper permissions preserved
FIRMWARE_ZIP="../nanokvm-rs-firmware.zip"
rm -f "$FIRMWARE_ZIP"
zip -r "$FIRMWARE_ZIP" kvmapp/

cd ..

echo ""
echo "=========================================="
echo "  Firmware package created successfully!"
echo "=========================================="
echo ""
echo "Output: nanokvm-rs-firmware.zip"
echo "Size:   $(ls -lh nanokvm-rs-firmware.zip | awk '{print $5}')"
echo ""
echo "Contents:"
unzip -l nanokvm-rs-firmware.zip | tail -20
echo ""
echo "To flash to device:"
echo "  1. Web UI: Settings -> System -> Update -> Upload ZIP"
echo "  2. SSH:    scp nanokvm-rs-firmware.zip root@nanokvm.local:/tmp/"
echo "             ssh root@nanokvm.local 'cd / && unzip -o /tmp/nanokvm-rs-firmware.zip'"
echo ""
