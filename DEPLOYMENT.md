# NanoKVM-RS Deployment Guide

This guide explains how to build and deploy the NanoKVM-RS firmware to your NanoKVM device.

## Prerequisites

### For Local Development/Building

- **Rust toolchain** with riscv64gc-unknown-linux-musl target:
  ```bash
  rustup target add riscv64gc-unknown-linux-musl
  ```

- **RISC-V cross-compiler** (for linking):
  - Download Sophgo's musl toolchain or build your own
  - Ensure `riscv64-unknown-linux-musl-gcc` is in PATH

- **Original NanoKVM sources** (for libraries and init scripts):
  ```bash
  git clone https://github.com/sipeed/NanoKVM ../NanoKVM
  ```

### For CI Builds

The GitHub Actions workflow handles all prerequisites automatically.

## Building Firmware Package

### One-Time Setup

Run the bootstrap script to copy libraries and init scripts from NanoKVM:

```bash
./bootstrap-deploy.sh
```

This creates the `deploy/` directory structure with:
- All shared libraries (libkvm.so, libvenc.so, etc.)
- Init scripts for hardware setup
- Service startup script

### Build Firmware

```bash
./deploy.sh
```

This will:
1. Build the Rust binary for RISC-V
2. Copy binary to deploy directory
3. Create `nanokvm-rs-firmware.zip`

Output: `nanokvm-rs-firmware.zip` (~5-10MB)

## Flashing to Device

### Method 1: Web UI Update (Recommended)

1. Access device web UI: `http://nanokvm.local` or `http://<device-ip>`
2. Navigate to **Settings** > **System** > **Update**
3. Upload `nanokvm-rs-firmware.zip`
4. Device will restart automatically

### Method 2: SSH/SCP

```bash
# Copy firmware to device
scp nanokvm-rs-firmware.zip root@nanokvm.local:/tmp/

# SSH into device
ssh root@nanokvm.local

# Stop current service
/etc/init.d/S95nanokvm stop

# Extract firmware (overwrites /kvmapp)
cd /
unzip -o /tmp/nanokvm-rs-firmware.zip

# Set permissions
chmod +x /kvmapp/server/nanokvm-rs-server

# Start new service
/etc/init.d/S95nanokvm-rs start
```

### Method 3: SD Card

1. Power off the NanoKVM device
2. Remove SD card and mount on your computer
3. Extract ZIP contents to `/kvmapp/` directory
4. Ensure executable permissions: `chmod +x /kvmapp/server/nanokvm-rs-server`
5. Reinsert SD card and power on

## Verifying Deployment

```bash
ssh root@nanokvm.local

# Check service is running
ps aux | grep nanokvm-rs-server

# Check library loading
ldd /kvmapp/server/nanokvm-rs-server

# Check logs
logread | grep -i nanokvm

# Test health endpoint
curl http://localhost/api/health
```

## Directory Structure

After deployment, the device will have:

```
/kvmapp/
├── server/
│   ├── nanokvm-rs-server      # Main binary
│   └── dl_lib/                # Shared libraries
│       ├── libkvm.so          # Video capture
│       ├── libvenc.so         # Video encoder
│       └── [30+ other .so]
└── system/
    ├── init.d/
    │   ├── S00kmod            # Kernel modules
    │   ├── S03usbdev          # USB gadget (HID)
    │   ├── S15kvmhwd          # GPIO/hardware
    │   ├── S95nanokvm-rs      # Our service
    │   └── [other init scripts]
    └── ko/
        └── soph_mipi_rx.ko    # MIPI driver module
```

## Troubleshooting

### Binary won't start

```bash
# Check if library dependencies are met
LD_LIBRARY_PATH=/kvmapp/server/dl_lib ldd /kvmapp/server/nanokvm-rs-server

# Try running manually with verbose output
cd /kvmapp/server
LD_LIBRARY_PATH=./dl_lib ./nanokvm-rs-server
```

### USB HID devices missing (/dev/hidg*)

```bash
# Reinitialize USB gadget
/etc/init.d/S03usbdev restart

# Verify devices exist
ls -l /dev/hidg*
```

### GPIO not working (power/reset buttons)

```bash
# Reinitialize hardware
/etc/init.d/S15kvmhwd restart

# Check GPIO exports
ls /sys/class/gpio/
cat /sys/class/gpio/gpio503/direction
```

### Video capture not working

```bash
# Check if libkvm.so is loaded
ldd /tmp/server/nanokvm-rs-server | grep kvm

# Check HDMI input detection
dmesg | grep -i hdmi
```

### Service fails to start on boot

```bash
# Check init script permissions
ls -la /etc/init.d/S95nanokvm-rs

# Make executable if needed
chmod +x /etc/init.d/S95nanokvm-rs

# Check for errors
/etc/init.d/S95nanokvm-rs start
```

## Reverting to Original Firmware

To revert to the original Go-based NanoKVM:

1. Stop the Rust service:
   ```bash
   /etc/init.d/S95nanokvm-rs stop
   ```

2. Download original firmware from Sipeed
3. Flash using web UI or SD card method

## Development Notes

### Building Locally (without cross-compiler)

For development/testing on x86_64:

```bash
# Build for native target (won't have real libkvm.so)
cargo build --release --target x86_64-unknown-linux-gnu

# Or on Windows
cargo build --release --target x86_64-pc-windows-msvc
```

### Stub Library Mode

When cross-compiling without the RISC-V toolchain (CI builds), a stub library is used:

```bash
export KVM_STUB=1
cargo build --release --target riscv64gc-unknown-linux-musl
```

The stub provides symbols for linking; the real `libkvm.so` is loaded at runtime on the device.

### Testing Individual Components

```bash
# Test just the HID module
cargo test -p hid

# Test storage module
cargo test -p storage
```
