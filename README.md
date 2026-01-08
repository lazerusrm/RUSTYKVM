# NanoKVM-RS

A high-performance Rust implementation of the [NanoKVM](https://github.com/sipeed/NanoKVM) server software for Sipeed's NanoKVM hardware.

## Overview

NanoKVM-RS is a complete rewrite of the original Go-based NanoKVM server in Rust, providing improved performance, memory safety, and modern async architecture while maintaining full compatibility with NanoKVM hardware.

## Features

### Core Functionality
- **Video Streaming** - H.264 hardware-accelerated video capture via WebRTC (WHEP protocol)
- **HID Emulation** - Full keyboard and mouse control over USB HID gadget
- **Virtual Media** - Mount ISO images as virtual CD-ROM drives
- **Terminal Access** - Web-based PTY terminal for direct device access
- **GPIO Control** - Hardware power button, reset, and LED control

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

### Production Features
- **Graceful shutdown** - Clean termination on SIGTERM/SIGINT
- **Health endpoint** - `/health` for load balancer checks (unauthenticated)
- **Structured logging** - Tracing with configurable log levels
- **Error handling** - Comprehensive error types with context

## Building

### Prerequisites
- Rust toolchain (stable)
- Docker (recommended for cross-compilation)

### Using Docker (Recommended)
```bash
# Build deployment package
make build

# Create release tarball
make package
```

### Native Cross-Compilation
```bash
# Install RISC-V target
rustup target add riscv64gc-unknown-linux-gnu

# Install cross-compiler
sudo apt install gcc-riscv64-linux-gnu g++-riscv64-linux-gnu

# Build
cargo build --release --target riscv64gc-unknown-linux-gnu
```

### CI/CD
The project includes GitHub Actions workflows that automatically:
- Run formatting and linting checks
- Cross-compile for RISC-V
- Create deployment packages
- Publish releases on version tags

## Installation

### On NanoKVM Device

1. Download the latest release or CI artifact
2. Copy to your NanoKVM:
   ```bash
   scp nanokvm-rs-*.tar.gz root@<nanokvm-ip>:/tmp/
   ```
3. SSH and install:
   ```bash
   ssh root@<nanokvm-ip>
   cd /tmp
   tar -xzf nanokvm-rs-*.tar.gz
   ./install.sh
   ```

The install script will:
- Backup existing installation
- Install binary and libraries to `/kvmapp/`
- Configure library paths
- Set up init script for auto-start
- Generate TLS certificates
- Start the service

### Manual Installation

```bash
# Copy files
cp nanokvm-server /kvmapp/
cp -r dl_lib/* /kvmapp/dl_lib/
cp -r web /kvmapp/

# Configure library path
echo "/kvmapp/dl_lib" > /etc/ld.so.conf.d/nanokvm.conf
ldconfig

# Start
cd /kvmapp && ./nanokvm-server
```

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
├── kvm/             # Video capture abstraction
├── kvm-sys/         # FFI bindings to libkvm hardware library
├── hid/             # USB HID gadget control
├── vm/              # GPIO and hardware control
├── storage/         # Virtual media (ISO mounting)
├── network/         # Network configuration
├── audio/           # Audio capture (optional)
├── web/             # Static web assets
├── dl_lib/          # Proprietary hardware libraries
└── scripts/         # Installation scripts
```

### Key Components

- **WebRTC Streaming** - WHEP endpoint at `/api/webrtc/whep` with WebSocket signaling
- **HID Input** - Processes keyboard/mouse events via `/dev/hidg*` devices
- **Video Capture** - Hardware H.264 encoding via `libkvm.so`
- **Authentication** - JWT-based sessions with bcrypt password hashing

## API Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/auth/login` | POST | Authenticate and get session |
| `/api/auth/logout` | POST | End session |
| `/api/webrtc/whep` | POST | WebRTC WHEP negotiation |
| `/api/ws/h264` | WS | WebRTC signaling WebSocket |
| `/api/hid/keyboard` | POST | Send keyboard input |
| `/api/hid/mouse` | POST | Send mouse input |
| `/api/vm/gpio/*` | GET/POST | GPIO control |
| `/api/storage/iso` | GET/POST | ISO management |
| `/api/terminal` | WS | PTY terminal WebSocket |
| `/health` | GET | Health check (no auth) |

## Hardware Libraries

The `dl_lib/` directory contains proprietary libraries from Sipeed/Sophgo for hardware access:

- `libkvm.so` - HDMI capture and H.264 encoding
- `libcvi_*.so` - Cvitek ISP and video processing
- `libtinyalsa.so` - Audio capture

These are required for video functionality on actual NanoKVM hardware. For CI builds, stub implementations are used.

## Development

```bash
# Format code
cargo fmt --all

# Run lints
cargo clippy --workspace

# Run tests (uses stubs, no hardware needed)
CI=1 cargo test --workspace

# Check without building
cargo check --workspace
```

## License

This project is licensed under the **GNU General Public License v3.0** - see [LICENSE](LICENSE) for details.

## Attribution

- **[Sipeed](https://sipeed.com)** - Original NanoKVM hardware and software
- **[Sophgo/Cvitek](https://www.sophgo.com)** - SG2002 SoC and libraries

See [NOTICE](NOTICE) for full attribution details.

## Contributing

Contributions are welcome! Please ensure:
- Code passes `cargo fmt` and `cargo clippy`
- New features include appropriate tests
- Commits are signed off (DCO)

## Support

- Issues: [GitHub Issues](https://github.com/lazerusrm/RUSTYKVM/issues)
- Original NanoKVM: [Sipeed NanoKVM](https://github.com/sipeed/NanoKVM)
