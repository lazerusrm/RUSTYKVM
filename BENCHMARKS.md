# NanoKVM-RS Performance

**Hardware:** NanoKVM (Sipeed SG2002 RISC-V SoC, 158 MB RAM)
**Test Date:** January 2026

## Rust vs Go Comparison

| Metric | Go | Rust | Improvement |
|--------|-----|------|-------------|
| **RAM Usage** | 24 MB | 14.5 MB | **40% less** |
| **Binary Size** | 19.6 MB | 11.6 MB | 41% smaller |
| **Input Latency** | 30-80ms | 13-16ms | **3-4x faster** |
| **Video Latency** | - | 29-47ms | Time to first frame |
| **Video Throughput** | - | 4.1 MB/s | 1080p @ 25fps |
| **CPU During Video** | - | 0% | Hardware encoder |

## Input Latency

Measured from HTTP request to USB HID report delivery:

| Operation | Latency |
|-----------|---------|
| Keyboard keystroke | 13-14ms |
| Mouse click | 13-16ms |
| Mouse movement | ~15ms |

Add ~5-10ms for network RTT on LAN.

## Video Streaming

### MJPEG (Default)

| Metric | Value |
|--------|-------|
| Time to First Frame | 29-47ms |
| Throughput | 4.1 MB/s |
| Frame Rate | ~25 fps |
| Frame Size | ~164 KB |

### H.264 (WebSocket)

| Metric | Value |
|--------|-------|
| Time to First Frame | 57-99ms |
| Throughput | 0.14 MB/s |
| Frame Rate | ~22 fps |

**MJPEG** is better for local use (lower latency). **H.264** uses 30x less bandwidth for remote connections.

## Latency Optimizations Applied

1. **TCP_NODELAY** - Disables Nagle's algorithm for immediate packet delivery
2. **Reduced broadcast buffers** (16 to 4) - Less frame queuing
3. **Reduced HID timeout** (10ms to 5ms) - Faster USB delivery
4. **Optimized frame headers** - BytesMut + itoa instead of format strings

## Test Methodology

- Local LAN with ~5-10ms base RTT
- HDMI source: Mac M4 mini @ 1920x1080 60Hz
- Measurements via `curl` with timing metrics
