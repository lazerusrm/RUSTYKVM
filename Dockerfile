# NanoKVM-RS Build Environment
# Multi-stage build for RISC-V cross-compilation

FROM ubuntu:24.04 AS builder

# Prevent interactive prompts
ENV DEBIAN_FRONTEND=noninteractive

# Install build dependencies
RUN apt-get update && apt-get install -y \
    curl \
    build-essential \
    gcc-riscv64-linux-gnu \
    g++-riscv64-linux-gnu \
    pkg-config \
    libssl-dev \
    ca-certificates \
    git \
    && rm -rf /var/lib/apt/lists/*

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"

# Add RISC-V target
RUN rustup target add riscv64gc-unknown-linux-gnu

# Set up cross-compilation environment
ENV CC_riscv64gc_unknown_linux_gnu=riscv64-linux-gnu-gcc
ENV CXX_riscv64gc_unknown_linux_gnu=riscv64-linux-gnu-g++
ENV CARGO_TARGET_RISCV64GC_UNKNOWN_LINUX_GNU_LINKER=riscv64-linux-gnu-gcc
ENV AR_riscv64gc_unknown_linux_gnu=riscv64-linux-gnu-ar

# Create build directory
WORKDIR /build

# Copy Cargo files first for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY server/Cargo.toml server/
COPY kvm-sys/Cargo.toml kvm-sys/
COPY hid/Cargo.toml hid/
COPY vm/Cargo.toml vm/
COPY audio/Cargo.toml audio/
COPY storage/Cargo.toml storage/
COPY .cargo .cargo

# Create dummy source files for dependency compilation
RUN mkdir -p server/src kvm-sys/src hid/src vm/src audio/src storage/src \
    && echo "fn main() {}" > server/src/main.rs \
    && echo "pub fn dummy() {}" > kvm-sys/src/lib.rs \
    && echo "pub fn dummy() {}" > hid/src/lib.rs \
    && echo "pub fn dummy() {}" > vm/src/lib.rs \
    && echo "pub fn dummy() {}" > audio/src/lib.rs \
    && echo "pub fn dummy() {}" > storage/src/lib.rs

# Build dependencies (this layer gets cached)
RUN cargo build --release --target riscv64gc-unknown-linux-gnu 2>/dev/null || true

# Now copy the real source code
COPY . .

# Build the actual project
RUN cargo build --release --target riscv64gc-unknown-linux-gnu

# Create deployment package
FROM ubuntu:24.04 AS packager

RUN apt-get update && apt-get install -y tar && rm -rf /var/lib/apt/lists/*

WORKDIR /package

# Copy binary
COPY --from=builder /build/target/riscv64gc-unknown-linux-gnu/release/nanokvm-server ./

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
