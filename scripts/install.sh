#!/bin/sh
# NanoKVM-RS Installation Script
# This script installs the Rust-based NanoKVM server on the device

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

log_info "Starting NanoKVM-RS installation..."

# Create backup directory
mkdir -p "$BACKUP_DIR"

# Stop existing service if running
if [ -f "/etc/init.d/$SERVICE_NAME" ]; then
    log_info "Stopping existing NanoKVM service..."
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

# Install proprietary libraries
if [ -d "$SCRIPT_DIR/dl_lib" ]; then
    log_info "Installing hardware libraries..."
    mkdir -p "$INSTALL_DIR/dl_lib"
    cp "$SCRIPT_DIR/dl_lib/"*.so "$INSTALL_DIR/dl_lib/"

    # Add to library path
    if ! grep -q "/kvmapp/dl_lib" /etc/ld.so.conf.d/nanokvm.conf 2>/dev/null; then
        echo "/kvmapp/dl_lib" > /etc/ld.so.conf.d/nanokvm.conf
        ldconfig 2>/dev/null || true
    fi
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
# NanoKVM-RS Configuration
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

# Create init script
log_info "Installing init script..."
cat > "/etc/init.d/$SERVICE_NAME" << 'EOF'
#!/bin/sh
### BEGIN INIT INFO
# Provides:          nanokvm
# Required-Start:    $network $remote_fs
# Required-Stop:     $network $remote_fs
# Default-Start:     2 3 4 5
# Default-Stop:      0 1 6
# Short-Description: NanoKVM Server
# Description:       Rust-based NanoKVM server for remote KVM access
### END INIT INFO

DAEMON=/kvmapp/nanokvm-server
DAEMON_ARGS=""
PIDFILE=/var/run/nanokvm.pid
LOGFILE=/var/log/nanokvm.log

export LD_LIBRARY_PATH=/kvmapp/dl_lib:$LD_LIBRARY_PATH

is_running() {
    [ -f "$PIDFILE" ] || return 1
    PID="$(cat "$PIDFILE" 2>/dev/null)"
    [ -n "$PID" ] || return 1
    kill -0 "$PID" 2>/dev/null
}

case "$1" in
    start)
        echo "Starting NanoKVM server..."
        cd /kvmapp
        start-stop-daemon -S -b -m -p $PIDFILE -a $DAEMON -- $DAEMON_ARGS >> $LOGFILE 2>&1
        sleep 1
        if ! is_running; then
            echo "NanoKVM server failed to start. Last 60 log lines:" >&2
            tail -n 60 "$LOGFILE" 2>/dev/null >&2 || true
            exit 1
        fi
        echo "NanoKVM server started"
        ;;
    stop)
        echo "Stopping NanoKVM server..."
        start-stop-daemon -K -p $PIDFILE -s TERM
        for i in 1 2 3 4 5 6 7 8 9 10; do
            is_running || break
            sleep 0.5
        done
        rm -f $PIDFILE
        echo "NanoKVM server stopped"
        ;;
    restart)
        $0 stop
        sleep 1
        $0 start
        ;;
    status)
        if [ -f $PIDFILE ] && kill -0 $(cat $PIDFILE) 2>/dev/null; then
            echo "NanoKVM server is running (PID: $(cat $PIDFILE))"
        else
            echo "NanoKVM server is not running"
            exit 1
        fi
        ;;
    *)
        echo "Usage: $0 {start|stop|restart|status}"
        exit 1
        ;;
esac

exit 0
EOF

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
            -subj "/CN=nanokvm/O=NanoKVM/C=US" 2>/dev/null
        log_info "TLS certificate generated"
    else
        log_warn "OpenSSL not found, skipping certificate generation"
        log_warn "You will need to provide server.crt and server.key manually"
    fi
fi

# Start the service
log_info "Starting NanoKVM service..."
/etc/init.d/$SERVICE_NAME start

log_info "Installation complete!"
log_info ""
log_info "The NanoKVM server is now running."
log_info "Access it at: https://$(hostname -I | awk '{print $1}')"
log_info ""
log_info "Configuration file: $CONFIG_DIR/config.yaml"
log_info "Log file: /var/log/nanokvm.log"
log_info ""
log_info "To check status: /etc/init.d/$SERVICE_NAME status"
log_info "To view logs: tail -f /var/log/nanokvm.log"
