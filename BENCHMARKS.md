# NanoKVM-RS Performance

**Hardware:** NanoKVM (Sipeed SG2002 RISC-V SoC, 158 MB RAM)
**HDMI Source:** Mac M4 mini @ 1920x1080 60Hz
**Network:** Local LAN, ~5-10ms base RTT

## Key Metrics

| Metric | Go | Rust | Notes |
|--------|-----|------|-------|
| **Input Latency** | 30-80ms | **13-16ms** | 3-4x faster |
| **Video TTFB** | ~50ms* | **29-47ms** | Time to first frame |
| **RAM Usage** | 24 MB | **14.5 MB** | 40% less |
| **Binary Size** | 19.6 MB | 11.6 MB | 41% smaller |

*Go TTFB estimated from similar hardware tests

## Video Streaming

| Stream Type | Time to First Frame | Throughput | Best For |
|-------------|--------------------:|------------|----------|
| **MJPEG** | 29-47ms | 4.1 MB/s @ 25fps | Local/LAN |
| **H.264** | 57-99ms | 0.14 MB/s @ 22fps | Remote/WAN |

MJPEG has lower latency. H.264 uses 30x less bandwidth.

## Input Latency Breakdown

| Operation | Rust Latency |
|-----------|-------------:|
| Keyboard keystroke | 13-14ms |
| Mouse click | 13-16ms |
| Mouse movement | ~15ms |

Add network RTT (~5-10ms on LAN) for total perceived latency.

## Optimizations Applied

- **TCP_NODELAY** - Immediate packet delivery (no Nagle buffering)
- **Reduced buffers** - 4 frame slots instead of 16
- **Faster HID** - 5ms timeout instead of 10ms
