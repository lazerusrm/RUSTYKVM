#!/bin/sh
# RUSTYKVM on-device installation
# Installs nanokvm-server, web assets, and the platform init script

set -e

INSTALL_DIR="/kvmapp"
BACKUP_DIR="/root/old"
CONFIG_DIR="/etc/kvm"
SERVICE_NAME="S95nanokvm"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo "${RED}[ERROR]${NC} $1"
}

# Check if running as root
if [ "$(id -u)" != "0" ]; then
    log_error "This script must be run as root"
    exit 1
fi

# Determine script directory
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Check for required files
if [ ! -f "$SCRIPT_DIR/nanokvm-server" ]; then
    log_error "nanokvm-server binary not found in $SCRIPT_DIR"
    exit 1
fi

log_info "Starting RUSTYKVM installation..."

# Create backup directory
mkdir -p "$BACKUP_DIR"

# Stop existing service if running
if [ -f "/etc/init.d/$SERVICE_NAME" ]; then
    log_info "Stopping existing RUSTYKVM service..."
    /etc/init.d/$SERVICE_NAME stop 2>/dev/null || true
fi

# Backup existing installation
if [ -d "$INSTALL_DIR" ]; then
    log_info "Backing up existing installation to $BACKUP_DIR..."
    BACKUP_NAME="kvmapp_backup_$(date +%Y%m%d_%H%M%S)"
    cp -r "$INSTALL_DIR" "$BACKUP_DIR/$BACKUP_NAME" 2>/dev/null || true
fi

# Create installation directory
mkdir -p "$INSTALL_DIR"
mkdir -p "$CONFIG_DIR"

# Install the binary
log_info "Installing nanokvm-server binary..."
cp "$SCRIPT_DIR/nanokvm-server" "$INSTALL_DIR/nanokvm-server"
chmod +x "$INSTALL_DIR/nanokvm-server"

if [ -f "$SCRIPT_DIR/RELEASE_VERSION" ]; then
    cp "$SCRIPT_DIR/RELEASE_VERSION" "$INSTALL_DIR/version"
elif [ -n "${NANOKVM_RELEASE_VERSION:-}" ]; then
    echo "$NANOKVM_RELEASE_VERSION" > "$INSTALL_DIR/version"
fi

# Install proprietary libraries (prefer release bundle dl_lib/, else platform tree)
log_info "Installing hardware libraries..."
mkdir -p "$INSTALL_DIR/dl_lib"
if [ -d "$SCRIPT_DIR/dl_lib" ] && ls "$SCRIPT_DIR/dl_lib/"*.so >/dev/null 2>&1; then
    cp "$SCRIPT_DIR/dl_lib/"*.so "$INSTALL_DIR/dl_lib/"
elif [ -d "$INSTALL_DIR/server/dl_lib" ]; then
    cp "$INSTALL_DIR/server/dl_lib/"*.so "$INSTALL_DIR/dl_lib/"
fi
if [ -f "$INSTALL_DIR/dl_lib/libkvm.so" ]; then
    if ! grep -q "/kvmapp/dl_lib" /etc/ld.so.conf.d/nanokvm.conf 2>/dev/null; then
        echo "/kvmapp/dl_lib" > /etc/ld.so.conf.d/nanokvm.conf
        ldconfig 2>/dev/null || true
    fi
else
    log_warn "libkvm.so not installed — video capture will not work until libraries are present"
fi

# Install web assets if present
if [ -d "$SCRIPT_DIR/web" ]; then
    log_info "Installing web assets..."
    rm -rf "$INSTALL_DIR/web"
    cp -r "$SCRIPT_DIR/web" "$INSTALL_DIR/web"
fi

# Create default config if it doesn't exist
if [ ! -f "$CONFIG_DIR/config.yaml" ]; then
    log_info "Creating default configuration..."
    cat > "$CONFIG_DIR/config.yaml" << 'EOF'
# RUSTYKVM configuration
# This file is auto-generated on first run if not present

http:
  port: 80

https:
  enabled: true
  port: 443
  cert: "server.crt"
  key: "server.key"

# Authentication settings
auth:
  # Session timeout in seconds (default: 24 hours)
  session_timeout: 86400

# CORS settings (configure for your environment)
cors:
  # Set to your web UI origin in production
  # allowed_origins: ["https://your-domain.com"]

# WebRTC STUN/TURN servers
webrtc:
  stun_servers:
    - "stun:stun.l.google.com:19302"
EOF
fi

# Install platform init script (single source: scripts/S95nanokvm in the release bundle)
log_info "Installing init script..."
if [ ! -f "$SCRIPT_DIR/S95nanokvm" ]; then
    log_error "S95nanokvm not found in $SCRIPT_DIR (include it in the release tarball)"
    exit 1
fi
cp "$SCRIPT_DIR/S95nanokvm" "/etc/init.d/$SERVICE_NAME"
chmod +x "/etc/init.d/$SERVICE_NAME"

# Enable the service
log_info "Enabling service to start on boot..."
if command -v update-rc.d >/dev/null 2>&1; then
    update-rc.d $SERVICE_NAME defaults
elif [ -d /etc/rc.d ]; then
    ln -sf "/etc/init.d/$SERVICE_NAME" /etc/rc.d/S95nanokvm
fi

# Generate self-signed certificate if not present
if [ ! -f "$INSTALL_DIR/server.crt" ] || [ ! -f "$INSTALL_DIR/server.key" ]; then
    log_info "Generating self-signed TLS certificate..."
    if command -v openssl >/dev/null 2>&1; then
        openssl req -x509 -newkey rsa:2048 -keyout "$INSTALL_DIR/server.key" \
            -out "$INSTALL_DIR/server.crt" -days 365 -nodes \
            -subj "/CN=rustykvm/O=RUSTYKVM/C=US" 2>/dev/null
        log_info "TLS certificate generated"
    else
        log_warn "OpenSSL not found, skipping certificate generation"
        log_warn "You will need to provide server.crt and server.key manually"
    fi
fi

# Start the service
log_info "Starting RUSTYKVM service..."
/etc/init.d/$SERVICE_NAME start

log_info "Installation complete!"
log_info ""
log_info "RUSTYKVM (nanokvm-server) is now running."
log_info "Access it at: https://$(hostname -I | awk '{print $1}')"
log_info ""
log_info "Configuration file: $CONFIG_DIR/config.yaml"
log_info "Log file: /var/log/nanokvm.log"
log_info ""
log_info "To check status: /etc/init.d/$SERVICE_NAME status"
log_info "To view logs: tail -f /var/log/nanokvm.log"
