# Hardware Interface Libraries

This directory contains pre-compiled binary libraries for the NanoKVM hardware
(Sophgo SG2002 RISC-V SoC). These libraries provide hardware-level access to:

- **libkvm.so** - HDMI capture and H.264 video encoding
- **libkvm_mmf.so** - Media framework
- **libcvi_*.so** - Cvitek/Sophgo ISP and video processing
- **libtinyalsa.so** - Audio capture (ALSA interface)
- **libvenc.so / libvpu.so** - Hardware video encoder

## Source and Attribution

These libraries are provided by:

- **Sipeed** - NanoKVM hardware manufacturer
- **Sophgo/Cvitek** - SG2002 SoC vendor

The libraries are distributed as part of the [NanoKVM](https://github.com/sipeed/NanoKVM)
open-source project under the GPL-3.0 license.

## Original Source

These binaries were obtained from:
https://github.com/sipeed/NanoKVM/tree/main/server/dl_lib

## License

The binary libraries are provided for use with NanoKVM hardware. For licensing
questions regarding these specific binaries, please contact Sipeed or refer to
the original NanoKVM repository.

## Note

These libraries are required for video capture functionality on actual NanoKVM
hardware. For CI builds and testing, stub implementations are used instead
(see `kvm-sys/build.rs`).
