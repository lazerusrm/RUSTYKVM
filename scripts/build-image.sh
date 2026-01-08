#!/bin/bash
# NanoKVM-RS SD Card Image Builder
# Creates a flashable SD card image with the Rust-based NanoKVM server

set -e

# Configuration
SIPEED_IMAGE_URL="https://github.com/sipeed/NanoKVM/releases/download/v1.4.0/20250217_NanoKVM_Rev1_4_0.img.xz"
SIPEED_IMAGE_NAME="NanoKVM_Base.img"
OUTPUT_IMAGE="nanokvm-rs.img"
WORK_DIR="/tmp/nanokvm-build"
MOUNT_POINT="$WORK_DIR/mnt"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

# Check if running as root
if [ "$(id -u)" != "0" ]; then
    log_error "This script must be run as root (for loop device mounting)"
    exit 1
fi

# Check required tools
for tool in xz losetup mount umount parted; do
    if ! command -v $tool &> /dev/null; then
        log_error "Required tool '$tool' not found"
        exit 1
    fi
done

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Check for required files
if [ ! -f "$PROJECT_DIR/output/nanokvm-server" ] && [ ! -f "$1" ]; then
    log_error "nanokvm-server binary not found!"
    log_error "Either build first with 'make build' or provide path as argument"
    exit 1
fi

BINARY_PATH="${1:-$PROJECT_DIR/output/nanokvm-server}"
DL_LIB_PATH="$PROJECT_DIR/dl_lib"
WEB_PATH="$PROJECT_DIR/web"
INSTALL_SCRIPT="$PROJECT_DIR/scripts/install.sh"

log_info "NanoKVM-RS Image Builder"
log_info "========================"

# Create work directory
mkdir -p "$WORK_DIR"
mkdir -p "$MOUNT_POINT"

# Download base image if not cached
if [ ! -f "$WORK_DIR/$SIPEED_IMAGE_NAME" ]; then
    log_info "Downloading Sipeed NanoKVM base image..."
    curl -L -o "$WORK_DIR/base.img.xz" "$SIPEED_IMAGE_URL"

    log_info "Extracting base image..."
    xz -d -k "$WORK_DIR/base.img.xz"
    mv "$WORK_DIR/base.img" "$WORK_DIR/$SIPEED_IMAGE_NAME"
    rm -f "$WORK_DIR/base.img.xz"
fi

# Copy base image for modification
log_info "Creating working copy of image..."
cp "$WORK_DIR/$SIPEED_IMAGE_NAME" "$WORK_DIR/$OUTPUT_IMAGE"

# Find the rootfs partition (usually partition 2)
log_info "Setting up loop device..."
LOOP_DEV=$(losetup -f --show -P "$WORK_DIR/$OUTPUT_IMAGE")
log_info "Loop device: $LOOP_DEV"

# Wait for partition devices
sleep 2

# Find rootfs partition (try p2, then p1)
if [ -b "${LOOP_DEV}p2" ]; then
    ROOTFS_PART="${LOOP_DEV}p2"
elif [ -b "${LOOP_DEV}p1" ]; then
    ROOTFS_PART="${LOOP_DEV}p1"
else
    log_error "Could not find rootfs partition"
    losetup -d "$LOOP_DEV"
    exit 1
fi

log_info "Mounting rootfs from $ROOTFS_PART..."
mount "$ROOTFS_PART" "$MOUNT_POINT"

# Verify we have the right filesystem
if [ ! -d "$MOUNT_POINT/kvmapp" ]; then
    log_warn "Creating /kvmapp directory..."
    mkdir -p "$MOUNT_POINT/kvmapp"
fi

# Backup original Go server
if [ -f "$MOUNT_POINT/kvmapp/server/NanoKVM-Server" ]; then
    log_info "Backing up original Go server..."
    mv "$MOUNT_POINT/kvmapp/server/NanoKVM-Server" "$MOUNT_POINT/kvmapp/server/NanoKVM-Server.go.bak"
fi

# Install Rust server
log_info "Installing NanoKVM-RS server..."
cp "$BINARY_PATH" "$MOUNT_POINT/kvmapp/nanokvm-server"
chmod +x "$MOUNT_POINT/kvmapp/nanokvm-server"

# Install hardware libraries
if [ -d "$DL_LIB_PATH" ]; then
    log_info "Installing hardware libraries..."
    mkdir -p "$MOUNT_POINT/kvmapp/dl_lib"
    cp "$DL_LIB_PATH"/*.so "$MOUNT_POINT/kvmapp/dl_lib/"
fi

# Install web assets
if [ -d "$WEB_PATH" ]; then
    log_info "Installing web assets..."
    rm -rf "$MOUNT_POINT/kvmapp/web"
    cp -r "$WEB_PATH" "$MOUNT_POINT/kvmapp/web"
fi

# Create library path config
log_info "Configuring library paths..."
if [ -d "$MOUNT_POINT/etc/ld.so.conf.d" ]; then
    echo "/kvmapp/dl_lib" > "$MOUNT_POINT/etc/ld.so.conf.d/nanokvm.conf"
elif [ -f "$MOUNT_POINT/etc/ld.so.conf" ]; then
    grep -q "/kvmapp/dl_lib" "$MOUNT_POINT/etc/ld.so.conf" || echo "/kvmapp/dl_lib" >> "$MOUNT_POINT/etc/ld.so.conf"
fi

# Disable any existing NanoKVM init scripts
for script in "$MOUNT_POINT"/etc/init.d/S*kvm* "$MOUNT_POINT"/etc/init.d/S*NanoKVM*; do
    if [ -f "$script" ] && [ "$script" != "$MOUNT_POINT/etc/init.d/S95nanokvm" ]; then
        log_info "Disabling original init script: $script"
        mv "$script" "${script}.disabled" 2>/dev/null || true
    fi
done

# Create init script
INIT_SCRIPT="$MOUNT_POINT/etc/init.d/S95nanokvm"
log_info "Creating init script..."
cat > "$INIT_SCRIPT" << 'INITEOF'
#!/bin/sh
export LD_LIBRARY_PATH=/kvmapp/dl_lib:$LD_LIBRARY_PATH
DAEMON=/kvmapp/nanokvm-server
PIDFILE=/var/run/nanokvm.pid
LOGFILE=/var/log/nanokvm.log

case "$1" in
  start)
    echo "Starting NanoKVM-RS..."
    cd /kvmapp
    $DAEMON >> $LOGFILE 2>&1 &
    echo $! > $PIDFILE
    ;;
  stop)
    if [ -f $PIDFILE ]; then
      kill $(cat $PIDFILE) 2>/dev/null
      rm -f $PIDFILE
    fi
    ;;
  restart)
    $0 stop
    sleep 1
    $0 start
    ;;
  *)
    echo "Usage: $0 {start|stop|restart}"
    exit 1
    ;;
esac
exit 0
INITEOF
chmod +x "$INIT_SCRIPT"

# Create version marker
echo "NanoKVM-RS $(date +%Y%m%d)" > "$MOUNT_POINT/kvmapp/.version-rs"

# Sync and unmount
log_info "Syncing filesystem..."
sync

log_info "Unmounting..."
umount "$MOUNT_POINT"
losetup -d "$LOOP_DEV"

# Compress the image
log_info "Compressing image..."
xz -9 -k "$WORK_DIR/$OUTPUT_IMAGE"

# Move to output
OUTPUT_DIR="${2:-$PROJECT_DIR}"
mv "$WORK_DIR/$OUTPUT_IMAGE.xz" "$OUTPUT_DIR/"
mv "$WORK_DIR/$OUTPUT_IMAGE" "$OUTPUT_DIR/"

log_info "========================"
log_info "Image build complete!"
log_info ""
log_info "Output files:"
log_info "  $OUTPUT_DIR/$OUTPUT_IMAGE (raw)"
log_info "  $OUTPUT_DIR/$OUTPUT_IMAGE.xz (compressed)"
log_info ""
log_info "Flash with: sudo dd if=$OUTPUT_IMAGE of=/dev/sdX bs=4M status=progress"
log_info "Or use Balena Etcher / Rufus with the .xz file"
