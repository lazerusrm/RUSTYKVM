# NanoKVM Rust Port Progress Tracker

## 🟢 Completed Features

### Core Infrastructure
- [x] **Build System**: Docker cross-compilation for `riscv64gc-unknown-linux-gnu`.
- [x] **Workspace Structure**: Clean separation into crates (`server`, `kvm-sys`, `kvm`, `hid`, `vm`, `storage`, `network`).
- [x] **Web Server**: `Axum 0.8` integrated with `Tokio`.
- [x] **Static Files**: Serving frontend assets via `tower-http`.
- [x] **Configuration System**: YAML-based config loading with defaults and generated secrets.
- [x] **Logging**: File-based logging with daily rotation and stdout support.

### Video Streaming
- [x] **MJPEG**: Safe `Kvm` wrapper with zero-copy capture and dynamic quality/FPS.
- [x] **H.264 / WebRTC**:
    - [x] Bindings for hardware H.264 encoder.
    - [x] WebRTC transport using `webrtc-rs` with STUN/TURN support.
    - [x] WebSocket signaling compatible with NanoKVM frontend.
    - [x] **Audio Support**: Integrated hardware PCM capture (TinyALSA) and Opus encoding for WebRTC.
- [x] **Direct H.264**: Binary WebSocket stream for raw Annex-B frames with timestamps.
- [x] **Dynamic Controls**: On-the-fly adjustment of resolution, FPS, and bitrate.

### Input (HID)
- [x] **Keyboard**: 8-byte reports to `/dev/hidg0`.
- [x] **Mouse**: Relative and Absolute reporting.
- [x] **WebSocket**: Unified input handling from frontend (`/api/ws`).
- [x] **HTTP Paste**: `/api/hid/paste` with support for US/DE/FR keyboard layouts.
- [x] **Shortcuts**: Persistent storage and management of HID shortcuts.
- [x] **HID Mode**: Toggling between 'normal' and 'hid-only' modes.
- [x] **HID Reset**: PHY reset support for USB HID.

### Virtual Machine Control (VM)
- [x] **Power Control**: ATX Power/Reset via GPIO.
- [x] **Status**: Detecting power state via LEDs.
- [x] **Mouse Jiggler**: Background loop to prevent host sleep.
- [x] **Web Terminal**: PTY-backed terminal via WebSocket.
- [x] **Scripts**: Management and execution of `.sh` and `.py` scripts.
- [x] **OLED**: Configuration for sleep timeout.
- [x] **Virtual Devices**: Toggling USB RNDIS and Mass Storage functions.
- [x] **PCIe HDMI**: Control and status for the PCIe HDMI chip.

### Storage & ISO
- [x] **ISO Management**: CRUD operations for files in `/data`.
- [x] **USB Mass Storage**: Mounting ISOs as CD-ROM via Gadget ConfigFS.
- [x] **Download Service**: ISO upload, URL download, and status tracking.

### System & Network
- [x] **Auth**: JWT middleware, persistent account storage, and AES-256 password decryption.
- [x] **Network**: WiFi management and Wake-on-LAN.
- [x] **Tailscale**: Integration with Tailscale CLI (status, up, down, login, install).
- [x] **Update**: Full OTA update pipeline (Online & Offline upload) with backup/restore and preview toggle.
- [x] **HTTPS/TLS**: Support for port 443 with automatic redirection.
- [x] **Security**: HSTS, X-Frame-Options, and CORS headers.

---

## 🧪 Testing & Verification
- [x] **Compilation**: All modules compile for `riscv64gc-unknown-linux-gnu`.
- [ ] **Runtime**: Needs final verification on physical SG2002 hardware.