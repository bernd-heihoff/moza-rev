#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="moza-rev"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALL_DIR="$HOME/.local/bin"
SERVICE_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

if [ "$EUID" -eq 0 ]; then
    echo "Error: do not run this installer with sudo."
    echo "       It installs a systemd user service for the current user."
    exit 1
fi

if ! command -v cargo &>/dev/null; then
    echo "Error: cargo is not installed."
    exit 1
fi

if ! id -nG | tr ' ' '\n' | grep -qx dialout; then
    echo "Warning: user '$USER' is not in the dialout group."
    echo "         Serial access to the MOZA wheelbase may fail."
fi

# Prevent an old system service and the user service from competing for
# serial access and UDP ports.
if systemctl is-active --quiet "$SERVICE_NAME.service" 2>/dev/null \
    || systemctl is-enabled --quiet "$SERVICE_NAME.service" 2>/dev/null; then
    echo "Error: the old system-level $SERVICE_NAME.service is still active or enabled."
    echo
    echo "Disable it first with:"
    echo "  sudo systemctl disable --now $SERVICE_NAME.service"
    exit 1
fi

echo "Building release binary..."
cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"

echo "Stopping existing user service..."
systemctl --user stop "$SERVICE_NAME.service" 2>/dev/null || true

echo "Installing binary to $INSTALL_DIR..."
install -Dm0755 \
    "$SCRIPT_DIR/target/release/$SERVICE_NAME" \
    "$INSTALL_DIR/$SERVICE_NAME"

echo "Installing user service to $SERVICE_DIR..."
install -Dm0644 \
    "$SCRIPT_DIR/$SERVICE_NAME.service" \
    "$SERVICE_DIR/$SERVICE_NAME.service"

systemctl --user daemon-reload
systemctl --user enable "$SERVICE_NAME.service"
systemctl --user restart "$SERVICE_NAME.service"

echo "Done! User service is running."
echo "  Status:  systemctl --user status $SERVICE_NAME"
echo "  Logs:    journalctl --user -u $SERVICE_NAME -f"
echo "  Stop:    systemctl --user stop $SERVICE_NAME"
