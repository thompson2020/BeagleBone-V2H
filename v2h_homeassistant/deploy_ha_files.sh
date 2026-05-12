#!/bin/bash
# Deploy BeagleBone-related HA sensor and template YAML files to Home Assistant.
# Requires cifs-utils: sudo apt install cifs-utils
#
# Automations and dashboards must be applied manually in the HA UI:
#   Automations: Settings → Automations → import YAML from automations/
#   Dashboards:  Dashboard → Edit → Raw config editor → paste from dashboards/

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

echo "Copying sensors/..."
cp "$SCRIPT_DIR/sensors/"*.yaml "$MOUNT_POINT/sensors/"
echo "  $(ls "$SCRIPT_DIR/sensors/"*.yaml | wc -l) file(s) copied"

echo "Copying templates/..."
cp "$SCRIPT_DIR/templates/"*.yaml "$MOUNT_POINT/templates/"
echo "  $(ls "$SCRIPT_DIR/templates/"*.yaml | wc -l) file(s) copied"

echo ""
echo "Done. In HA: Developer Tools → YAML → Reload Template Entities"
echo "For REST sensor changes a full HA restart is required."
