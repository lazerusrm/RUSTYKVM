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
| `GET/POST /api/tailscale/auto-update` | Manage automatic Tailscale updates |

#### Tailscale Auto-Update

Enable or disable automatic Tailscale updates via the NanoKVM web interface:

**Get current auto-update status:**
```bash
curl https://<device-ip>/api/tailscale/auto-update
# Response: {"enabled": true}
```

**Enable auto-update:**
```bash
curl -X POST https://<device-ip>/api/tailscale/auto-update \
  -H "Content-Type: application/json" \
  -d '{"enabled": true}'
```

**Disable auto-update:**
```bash
curl -X POST https://<device-ip>/api/tailscale/auto-update \
  -H "Content-Type: application/json" \
  -d '{"enabled": false}'
```

**Features:**
- Automatic updates keep Tailscale client synchronized with latest security patches
- Settings persist across device reboots
- Disabled by default - explicitly enable to auto-update
- Available only when Tailscale is installed on the device

**Note:** This feature requires Tailscale to be installed and connected. Returns HTTP 503 if Tailscale is not available.

### Ethernet Network Configuration

Flexible network configuration supporting both DHCP and static IP modes with DNS management:

**Get current and saved network configuration:**
```bash
curl https://<device-ip>/api/network/ethernet
# Response example:
# {
#   "config": {
#     "dhcp": false,
#     "ip": "192.168.1.100",
#     "netmask": "255.255.255.0",
#     "gateway": "192.168.1.1",
#     "dns1": "8.8.8.8",
#     "dns2": "8.8.4.4"
#   },
#   "current": {
#     "dhcp": true,
#     "ip": "192.168.1.50",
#     "netmask": "255.255.255.0",
#     "gateway": "192.168.1.1",
#     "dns1": "192.168.1.1",
#     "dns2": null
#   }
# }
```

**Enable DHCP mode:**
```bash
curl -X POST https://<device-ip>/api/network/ethernet \
  -H "Content-Type: application/json" \
  -d '{"dhcp": true}'
```

**Configure static IP:**
```bash
curl -X POST https://<device-ip>/api/network/ethernet \
  -H "Content-Type: application/json" \
  -d '{
    "dhcp": false,
    "ip": "192.168.1.100",
    "netmask": "255.255.255.0",
    "gateway": "192.168.1.1",
    "dns1": "8.8.8.8",
    "dns2": "8.8.4.4"
  }'
```

**Configuration Details:**

| Field | Required | Format | Example |
|-------|----------|--------|---------|
| `dhcp` | Yes | boolean | `true` or `false` |
| `ip` | Static only | IPv4 address | `192.168.1.100` |
| `netmask` | Static only | Dotted decimal | `255.255.255.0` |
| `gateway` | Static only | IPv4 address | `192.168.1.1` |
| `dns1` | Optional | IPv4 address | `8.8.8.8` |
| `dns2` | Optional | IPv4 address | `8.8.4.4` |

**Features:**
- **DHCP Mode**: Automatic IP configuration from network DHCP server (recommended for most users)
- **Static IP Mode**: Manual IP configuration for fixed network presence (useful for servers)
- **DNS Configuration**: Override DHCP DNS servers with custom nameservers
- **Configuration Persistence**: Settings saved to `/etc/kvm/ethernet.yaml` and survive reboots
- **Input Validation**: All IP addresses, netmasks, and gateways are validated before application
- **Async Network Restart**: 500ms delay before network restart to allow API response to complete

**Validation Rules:**
- IP addresses must be valid IPv4 format (e.g., 192.168.1.100)
- Netmask must be a valid CIDR netmask in dotted decimal format
  - Valid: `255.255.255.0` (continuous 1-bits followed by 0-bits)
  - Invalid: `255.255.255.1` (non-continuous bit pattern)
- Gateway must be reachable on the configured subnet
- DNS servers are optional but must be valid IPv4 addresses if provided

**Implementation Details:**
- Saves configuration to `/boot/eth.nodhcp` (static IP marker file in CIDR format: `IP/PREFIX gateway`)
- Saves persistent config to `/etc/kvm/ethernet.yaml` (YAML format)
- Updates `/etc/resolv.conf` with DNS nameservers
- Executes `/etc/init.d/S30eth` to restart network service
- Returns HTTP 400 with error details if validation fails

**Troubleshooting:**
- If network becomes unreachable after static IP configuration, revert to DHCP and reconfigure
- DNS issues: Verify nameservers with `cat /etc/resolv.conf` on device
- Network not restarting: Check `/var/log/nanokvm_auth.log` for error details
- Configuration not persisting: Verify `/etc/kvm/` directory exists and has write permissions

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

### Performance & Memory Improvements

NanoKVM-RS delivers significant performance gains through Rust's zero-cost abstractions.

**Measured Performance (v0.2.0):**

| Metric | Original (Go) | NanoKVM-RS | Improvement |
|--------|---------------|------------|-------------|
| Physical RAM (RSS) | 24 MB | 14.5-16.5 MB | **40% less** |
| Virtual Memory | 1,252 MB | 42 MB | **30x less** |
| Binary Size | 19.6 MB | 11.6 MB | **41% smaller** |
| API Latency | 9-75ms | **13-21ms** | **2-3x faster** |
| HID Input Latency | ~50ms | **13-16ms** | **4x faster** |
| Video Throughput | 4.2 MB/s | 4.2 MB/s | Same (hardware) |

See [BENCHMARKS.md](BENCHMARKS.md) for detailed measurements.

**Latency Optimizations:**
- **TCP_NODELAY** - Immediate packet sending (no Nagle buffering)
- **Reduced broadcast buffers** - Minimizes frame queuing delay
- **Optimized HID timeouts** - 5ms write timeout for fast USB delivery
- **Zero-copy frame serialization** - BytesMut with itoa for headers

**Zero-Copy Architecture:**
- Video frames passed directly from hardware to WebRTC without intermediate copies
- Memory-mapped I/O for GPIO and HID operations
- Direct buffer sharing between capture and encoding pipelines
- No garbage collection pauses - deterministic memory management

**Async Performance:**
- Tokio runtime with work-stealing scheduler
- Lock-free data structures where possible
- Minimal context switching overhead
- True parallel I/O without thread pool bottlenecks

**Comparison with Original:**

| Feature | Original (Go) | NanoKVM-RS |
|---------|---------------|------------|
| Memory Safety | Runtime checks | Compile-time guarantees |
| Async Model | goroutines + GC | Tokio (zero-cost futures) |
| Web Framework | Gin | Axum (type-safe extractors) |
| Serialization | encoding/json | serde (zero-copy deserialize) |
| WebRTC Stack | Custom/pion | webrtc-rs (standards-compliant) |
| Error Handling | error returns | Result types with context |

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
| `/api/vm/tailscale/auto-update` | GET/POST | Tailscale auto-update toggle |
| `/api/storage/iso` | GET/POST | ISO management |
| `/api/storage/health` | GET | SD card health status |
| `/api/tailscale/*` | Various | Tailscale VPN management |
| `/api/network/ethernet` | GET/POST | Ethernet configuration (DHCP/static IP) |
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
