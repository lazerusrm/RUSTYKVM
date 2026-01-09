#!/bin/bash
# One-time setup to populate deploy/ from original NanoKVM sources
# Run this once before running deploy.sh

set -e

NANOKVM_PATH="${NANOKVM_PATH:-../NanoKVM}"

if [ ! -d "$NANOKVM_PATH" ]; then
    echo "ERROR: NanoKVM source not found at $NANOKVM_PATH"
    echo ""
    echo "Please either:"
    echo "  1. Set NANOKVM_PATH environment variable, or"
    echo "  2. Clone NanoKVM alongside nanokvm-rs:"
    echo "     git clone https://github.com/sipeed/NanoKVM ../NanoKVM"
    exit 1
fi

echo "==> Creating deploy directory structure..."
mkdir -p deploy/kvmapp/server/dl_lib
mkdir -p deploy/kvmapp/system/init.d

echo "==> Copying shared libraries from $NANOKVM_PATH/server/dl_lib/..."
cp -v "$NANOKVM_PATH/server/dl_lib"/*.so deploy/kvmapp/server/dl_lib/

echo "==> Copying system files from $NANOKVM_PATH/kvmapp/system/..."
cp -rv "$NANOKVM_PATH/kvmapp/system"/* deploy/kvmapp/system/

echo "==> Creating S95nanokvm-rs init script..."
cat > deploy/kvmapp/system/init.d/S95nanokvm-rs << 'INITSCRIPT'
#!/bin/sh
# NanoKVM-RS service init script
# This replaces the Go-based NanoKVM-Server with the Rust version

PIDFILE=/var/run/nanokvm-rs.pid
KVMAPP_SERVER=/kvmapp/server/nanokvm-rs-server

# Ensure shared libraries can be found
LD_LIBRARY_PATH=/kvmapp/server/dl_lib:/tmp/server/dl_lib:$LD_LIBRARY_PATH
export LD_LIBRARY_PATH

case "$1" in
    start)
        echo "Starting NanoKVM-RS server..."

        # Generate unique device key if not exists
        if [ ! -f /etc/kvm/key ]; then
            mkdir -p /etc/kvm
            head -c 32 /dev/urandom | base64 | head -c 32 > /etc/kvm/key
        fi

        # Copy server to tmpfs for faster execution on resource-constrained device
        rm -rf /tmp/server
        cp -r /kvmapp/server /tmp/

        # Start the server
        cd /tmp/server
        ./nanokvm-rs-server &
        echo $! > $PIDFILE

        echo "NanoKVM-RS server started (PID: $(cat $PIDFILE))"
        ;;

    stop)
        echo "Stopping NanoKVM-RS server..."
        if [ -f $PIDFILE ]; then
            kill $(cat $PIDFILE) 2>/dev/null || true
            rm -f $PIDFILE
        fi
        killall nanokvm-rs-server 2>/dev/null || true
        echo "NanoKVM-RS server stopped"
        ;;

    restart)
        $0 stop
        sleep 2
        $0 start
        ;;

    status)
        if [ -f $PIDFILE ] && kill -0 $(cat $PIDFILE) 2>/dev/null; then
            echo "NanoKVM-RS server is running (PID: $(cat $PIDFILE))"
            exit 0
        else
            echo "NanoKVM-RS server is not running"
            exit 1
        fi
        ;;

    *)
        echo "Usage: $0 {start|stop|restart|status}"
        exit 1
        ;;
esac

exit 0
INITSCRIPT

chmod +x deploy/kvmapp/system/init.d/S95nanokvm-rs

echo ""
echo "==> Bootstrap complete!"
echo ""
echo "Libraries copied: $(ls -1 deploy/kvmapp/server/dl_lib/*.so | wc -l)"
echo "Init scripts:     $(ls -1 deploy/kvmapp/system/init.d/ | wc -l)"
echo "Total size:       $(du -sh deploy/kvmapp/ | cut -f1)"
echo ""
echo "Next steps:"
echo "  1. Build the firmware package: ./deploy.sh"
echo "  2. Flash the device with:      nanokvm-rs-firmware.zip"
