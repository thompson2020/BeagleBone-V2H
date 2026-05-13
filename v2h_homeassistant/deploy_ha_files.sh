#!/bin/bash
# Deploy BeagleBone-related HA YAML files to Home Assistant.
# Requires cifs-utils: sudo apt install cifs-utils
#
# Dashboards must be applied manually in the HA UI:
#   Dashboard → Edit → Raw config editor → paste from dashboards/
#
# After deploy: Developer Tools → YAML → Check Config, then restart HA
# to pick up any changes to config/package_v2h_settings.yaml.

set -euo pipefail

SHARE="//192.168.10.21/config"
MOUNT_POINT="/mnt/ha_config"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

CREDS_FILE="$SCRIPT_DIR/.deploy_credentials"
if [[ ! -f "$CREDS_FILE" ]]; then
    echo "Error: credentials file not found at $CREDS_FILE"
    echo "Copy $SCRIPT_DIR/.deploy_credentials.example to $CREDS_FILE and fill in your details."
    exit 1
fi
source "$CREDS_FILE"

cleanup() {
    if mountpoint -q "$MOUNT_POINT"; then
        sudo umount "$MOUNT_POINT"
        echo "Unmounted $MOUNT_POINT"
    fi
}
trap cleanup EXIT

sudo mkdir -p "$MOUNT_POINT"

echo "Mounting $SHARE..."
sudo mount -t cifs "$SHARE" "$MOUNT_POINT" \
    -o "username=$SMB_USER,password=$SMB_PASS,uid=$(id -u),gid=$(id -g),file_mode=0644,dir_mode=0755"

mkdir -p "$MOUNT_POINT/sensors"
mkdir -p "$MOUNT_POINT/templates"
mkdir -p "$MOUNT_POINT/packages"

echo "Copying sensors/..."
cp "$SCRIPT_DIR/sensors/"*.yaml "$MOUNT_POINT/sensors/"
for f in "$SCRIPT_DIR/sensors/"*.yaml; do echo "  sensors/$(basename "$f")"; done

echo "Copying templates/..."
cp "$SCRIPT_DIR/templates/"*.yaml "$MOUNT_POINT/templates/"
for f in "$SCRIPT_DIR/templates/"*.yaml; do echo "  templates/$(basename "$f")"; done

echo "Copying packages/..."
cp "$SCRIPT_DIR/packages/"*.yaml "$MOUNT_POINT/packages/"
for f in "$SCRIPT_DIR/packages/"*.yaml; do echo "  packages/$(basename "$f")"; done

echo ""
echo "Done. To reload in HA:"
echo "  sensors/   → Developer Tools → YAML → Reload Template Entities"
echo "  templates/ → Developer Tools → YAML → Reload Template Entities"
echo "  packages/  → Developer Tools → YAML → Check Config, then restart HA"
