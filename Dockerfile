# NanoKVM-RS Build Environment
# Multi-stage build for RISC-V cross-compilation with musl libc
# The NanoKVM device runs musl, NOT glibc!

FROM ubuntu:24.04 AS builder

# Prevent interactive prompts
ENV DEBIAN_FRONTEND=noninteractive

# Install build dependencies
RUN apt-get update && apt-get install -y \
    curl \
    build-essential \
    cmake \
    pkg-config \
    libssl-dev \
    ca-certificates \
    git \
    xz-utils \
    wget \
    clang \
    && rm -rf /var/lib/apt/lists/*

# Download and install musl cross-compiler for RISC-V
# Using bootlin prebuilt toolchain
RUN mkdir -p /opt/riscv-musl && \
    wget -q https://toolchains.bootlin.com/downloads/releases/toolchains/riscv64-lp64d/tarballs/riscv64-lp64d--musl--stable-2024.05-1.tar.xz -O /tmp/toolchain.tar.xz && \
    tar -xf /tmp/toolchain.tar.xz -C /opt/riscv-musl --strip-components=1 && \
    rm /tmp/toolchain.tar.xz

ENV PATH="/opt/riscv-musl/bin:${PATH}"

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Add RISC-V musl target
RUN rustup target add riscv64gc-unknown-linux-musl

# Set up cross-compilation environment for musl
ENV CC_riscv64gc_unknown_linux_musl=riscv64-linux-gcc
ENV CXX_riscv64gc_unknown_linux_musl=riscv64-linux-g++
ENV CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_MUSL_LINKER=riscv64-linux-gcc
ENV AR_riscv64gc_unknown_linux_musl=riscv64-linux-ar

# Create build directory
WORKDIR /build

# Copy Cargo files first for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY server/Cargo.toml server/
COPY kvm-sys/Cargo.toml kvm-sys/
COPY kvm/Cargo.toml kvm/
COPY hid/Cargo.toml hid/
COPY vm/Cargo.toml vm/
COPY audio/Cargo.toml audio/
COPY storage/Cargo.toml storage/
COPY network/Cargo.toml network/
COPY .cargo .cargo

# Create dummy source files for dependency compilation
RUN mkdir -p server/src kvm-sys/src kvm/src hid/src vm/src audio/src storage/src network/src \
    && echo "fn main() {}" > server/src/main.rs \
    && echo "pub fn dummy() {}" > kvm-sys/src/lib.rs \
    && echo "pub fn dummy() {}" > kvm/src/lib.rs \
    && echo "pub fn dummy() {}" > hid/src/lib.rs \
    && echo "pub fn dummy() {}" > vm/src/lib.rs \
    && echo "pub fn dummy() {}" > audio/src/lib.rs \
    && echo "pub fn dummy() {}" > storage/src/lib.rs \
    && echo "pub fn dummy() {}" > network/src/lib.rs

# Build dependencies (this layer gets cached)
RUN cargo build --release --target riscv64gc-unknown-linux-musl 2>/dev/null || true

# Now copy the real source code
COPY . .

# Build the actual project
RUN cargo build --release --target riscv64gc-unknown-linux-musl

# Create deployment package
FROM ubuntu:24.04 AS packager

RUN apt-get update && apt-get install -y tar && rm -rf /var/lib/apt/lists/*

WORKDIR /package

# Copy binary
COPY --from=builder /build/target/riscv64gc-unknown-linux-musl/release/nanokvm-server ./

# Copy web assets
COPY web/ ./web/

# Copy proprietary libraries
COPY dl_lib/ ./dl_lib/

# Copy install script
COPY scripts/install.sh ./

# Make scripts executable
RUN chmod +x nanokvm-server install.sh

# Create tarball
RUN tar -czvf /nanokvm-rs.tar.gz .

# Final output stage - just the tarball
FROM scratch AS output
COPY --from=packager /nanokvm-rs.tar.gz /
