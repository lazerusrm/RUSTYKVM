# NanoKVM-RS

A high-performance Rust implementation of the [NanoKVM](https://github.com/sipeed/NanoKVM) server software for Sipeed's NanoKVM hardware.

## Overview

NanoKVM-RS is a complete rewrite of the original Go-based NanoKVM server in Rust, providing improved performance, memory safety, and modern async architecture while maintaining full compatibility with NanoKVM hardware.

## Features

### Core Functionality
- **Video Streaming** - H.264 hardware-accelerated video capture via WebRTC
- **HID Emulation** - Full keyboard and mouse control over USB HID gadget
- **Virtual Media** - Mount ISO images as virtual CD-ROM drives
- **Terminal Access** - Web-based PTY terminal for direct device access
- **GPIO Control** - Hardware power button, reset, and LED control

### Hardware Acceleration

NanoKVM-RS leverages the SG2002 SoC's dedicated hardware encoders for maximum performance:

| Component | Technology | Benefit |
|-----------|------------|---------|
| Video Capture | Hardware HDMI capture | Zero CPU overhead for video input |
| H.264 Encoding | Hardware VPU encoder | Real-time 1080p encoding at <5% CPU |
| ISP Processing | Cvitek ISP pipeline | Hardware image processing and scaling |
| Audio Capture | TinyALSA + hardware codec | Low-latency audio streaming |

The hardware acceleration is provided through `libkvm.so` and related libraries, enabling:
- **1080p @ 30fps** video streaming with minimal latency
- **< 100ms** end-to-end latency for keyboard/mouse input
- **Low power consumption** - entire device runs under 2W

### Tailscale Integration

Built-in Tailscale VPN support for secure remote access without port forwarding:

- **One-click install** - Downloads and installs Tailscale directly on device
- **Web-based login** - Authenticate via the NanoKVM web interface
- **Auto-start** - Tailscale service persists across reboots
- **Secure mesh** - Access your KVM from anywhere on your tailnet

API Endpoints:
| Endpoint | Description |
|----------|-------------|
| `POST /api/tailscale/install` | Download and install Tailscale |
| `POST /api/tailscale/login` | Get authentication URL |
| `POST /api/tailscale/up` | Connect to tailnet |
| `POST /api/tailscale/down` | Disconnect from tailnet |
| `GET /api/tailscale/status` | Get connection status |

### Passkey Authentication (WebAuthn)

Passwordless authentication using FIDO2/WebAuthn passkeys:

- **Hardware security keys** - YubiKey, Titan, and other FIDO2 devices
- **Platform authenticators** - Windows Hello, Touch ID, Face ID
- **QR code enrollment** - Easy setup from mobile devices
- **Recovery codes** - Backup access if passkey is lost
- **Challenge-response** - Cryptographic authentication immune to phishing

Features:
- 5-minute challenge expiration for security
- Multiple passkeys per account supported
- Secure credential storage on device
- Falls back to password if no passkey configured

### SD Card Health Monitoring

Proactive monitoring of the microSD card to prevent data loss:

**Metrics Tracked:**
- **Wear Level** - Flash cell degradation percentage
- **Pre-EOL Status** - Manufacturer end-of-life warning
- **I/O Errors** - Read/write error counts
- **Temperature** - Card operating temperature
- **Power Cycles** - Boot count tracking
- **Read/Write Stats** - Total data transferred

**Health Scoring:**
| Score | Status | Action |
|-------|--------|--------|
| 80-100 | Good | Normal operation |
| 50-79 | Fair | Consider backup |
| 20-49 | Warning | Replace soon |
| 0-19 | Fail | Replace immediately |

The health data is cached for 24 hours and accessible via `/api/storage/health`.

### Improvements Over Original

| Feature | Original (Go) | NanoKVM-RS |
|---------|---------------|------------|
| Memory Safety | Runtime checks | Compile-time guarantees |
| Async Runtime | goroutines | Tokio (zero-cost async) |
| Web Framework | Gin | Axum (type-safe, fast) |
| Binary Size | ~15MB | ~8MB (stripped) |
| Startup Time | ~2s | <500ms |
| WebRTC | Custom | webrtc-rs (standards-compliant) |

### Security Enhancements

- **Cryptographic secrets** - JWT keys auto-generated, never hardcoded
- **Password security** - Bcrypt hashing with configurable cost factor
- **HTTPS by default** - TLS 1.3 with auto-generated certificates
- **Session management** - Secure cookie handling with SameSite protection
- **Audit logging** - Authentication events logged to `/var/log/nanokvm_auth.log`
- **CORS protection** - Configurable origin restrictions
- **Passkey support** - Phishing-resistant WebAuthn authentication

### Production Features

- **Graceful shutdown** - Clean termination on SIGTERM/SIGINT
- **Health endpoint** - `/health` for load balancer checks (unauthenticated)
- **Structured logging** - Tracing with configurable log levels
- **Error handling** - Comprehensive error types with context

## Installation

### Quick Start

1. Download the latest SD card image from [Releases](https://github.com/lazerusrm/RUSTYKVM/releases)
2. Flash to a microSD card using your preferred tool:
   - **[Rufus](https://rufus.ie/)** (Windows)
   - **[balenaEtcher](https://etcher.balena.io/)** (Windows/Mac/Linux)
   - **[Raspberry Pi Imager](https://www.raspberrypi.com/software/)** (Windows/Mac/Linux)
3. Insert the SD card into your NanoKVM and power on
4. Access the web interface at `https://<device-ip>`

### Upgrading Existing NanoKVM

If you already have a NanoKVM running the original firmware:

1. Download the upgrade package from [Releases](https://github.com/lazerusrm/RUSTYKVM/releases)
2. Copy to your device: `scp nanokvm-rs-*.tar.gz root@<ip>:/tmp/`
3. SSH in and run: `cd /tmp && tar -xzf nanokvm-rs-*.tar.gz && ./install.sh`

The installer automatically backs up your existing configuration.

## Configuration

Configuration file: `/etc/kvm/config.yaml`

```yaml
http:
  port: 80

https:
  enabled: true
  port: 443
  cert: "server.crt"
  key: "server.key"

auth:
  session_timeout: 86400  # 24 hours

webrtc:
  stun_servers:
    - "stun:stun.l.google.com:19302"
```

## Architecture

```
nanokvm-rs/
├── server/          # Main HTTP/WebSocket server (Axum)
│   └── passkey/     # WebAuthn/FIDO2 authentication
├── kvm/             # Video capture abstraction
├── kvm-sys/         # FFI bindings to libkvm hardware library
├── hid/             # USB HID gadget control
├── vm/              # GPIO and hardware control
├── storage/         # Virtual media & SD health monitoring
├── network/         # Network configuration
├── audio/           # Audio capture (optional)
├── web/             # Static web assets
├── dl_lib/          # Hardware acceleration libraries
└── scripts/         # Installation scripts
```

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/auth/login` | POST | Authenticate and get session |
| `/api/auth/logout` | POST | End session |
| `/api/passkey/*` | Various | WebAuthn passkey operations |
| `/api/webrtc/whep` | POST | WebRTC WHEP negotiation |
| `/api/ws/h264` | WS | WebRTC signaling WebSocket |
| `/api/hid/keyboard` | POST | Send keyboard input |
| `/api/hid/mouse` | POST | Send mouse input |
| `/api/vm/gpio/*` | GET/POST | GPIO control |
| `/api/storage/iso` | GET/POST | ISO management |
| `/api/storage/health` | GET | SD card health status |
| `/api/tailscale/*` | Various | Tailscale VPN management |
| `/api/terminal` | WS | PTY terminal WebSocket |
| `/health` | GET | Health check (no auth) |

## Hardware Libraries

The `dl_lib/` directory contains hardware acceleration libraries from Sipeed/Sophgo:

| Library | Function |
|---------|----------|
| `libkvm.so` | HDMI capture and H.264 hardware encoding |
| `libkvm_mmf.so` | Media framework integration |
| `libcvi_ispd2.so` | Image Signal Processor daemon |
| `libvenc.so` / `libvpu.so` | Hardware video encoder |
| `libtinyalsa.so` | ALSA audio interface |

These provide zero-copy hardware acceleration for video streaming.

## License

This project is licensed under the **GNU General Public License v3.0** - see [LICENSE](LICENSE) for details.

## Attribution

- **[Sipeed](https://sipeed.com)** - Original NanoKVM hardware and software
- **[Sophgo/Cvitek](https://www.sophgo.com)** - SG2002 SoC and hardware libraries

See [NOTICE](NOTICE) for full attribution details.

## Contributing

Contributions are welcome! Please ensure:
- Code passes `cargo fmt` and `cargo clippy`
- New features include appropriate tests
- Commits are signed off (DCO)

## Support

- Issues: [GitHub Issues](https://github.com/lazerusrm/RUSTYKVM/issues)
- Original NanoKVM: [Sipeed NanoKVM](https://github.com/sipeed/NanoKVM)
