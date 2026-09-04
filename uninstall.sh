#!/usr/bin/env bash
set -euo pipefail

SERVICE_NAME="moza-rev"
INSTALL_DIR="$HOME/.local/bin"
SERVICE_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

if [ "$EUID" -eq 0 ]; then
    echo "Error: do not run this uninstaller with sudo."
    echo "       It manages the current user's systemd service."
    exit 1
fi

HAS_SERVICE=false
HAS_BINARY=false

if systemctl --user is-enabled "$SERVICE_NAME.service" &>/dev/null \
    || [ -f "$SERVICE_DIR/$SERVICE_NAME.service" ]; then
    HAS_SERVICE=true
fi

if [ -f "$INSTALL_DIR/$SERVICE_NAME" ]; then
    HAS_BINARY=true
fi

if [ "$HAS_SERVICE" = false ] && [ "$HAS_BINARY" = false ]; then
    echo "User service is not installed."
    echo "Binary is not installed."
    exit 0
fi

if [ "$HAS_SERVICE" = true ]; then
    echo "Stopping and disabling user service..."
    systemctl --user disable --now "$SERVICE_NAME.service" 2>/dev/null || true

    echo "Removing user service file..."
    rm -f "$SERVICE_DIR/$SERVICE_NAME.service"

    systemctl --user daemon-reload
    systemctl --user reset-failed "$SERVICE_NAME.service" 2>/dev/null || true
else
    echo "User service is not installed, skipping."
fi

if [ "$HAS_BINARY" = true ]; then
    echo "Removing binary..."
    rm -f "$INSTALL_DIR/$SERVICE_NAME"
else
    echo "Binary is not installed, skipping."
fi

echo "Done! User service has been removed."
