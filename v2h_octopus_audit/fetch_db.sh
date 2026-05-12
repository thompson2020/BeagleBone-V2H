#!/bin/bash
# Fetch the HA database — mounts the CIFS share and takes a safe online backup.
# Run this once before audit.sh; no need to re-run between test iterations.
#
# Prerequisites:  sudo apt install cifs-utils python3-venv
# Credentials:    ../v2h_homeassistant/.deploy_credentials  (SMB_USER / SMB_PASS)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SHARE="//192.168.10.21/config"
MOUNT_POINT="/mnt/ha_config"
HA_DB_SRC="$MOUNT_POINT/home-assistant_v2.db"
HA_DB_DST="$SCRIPT_DIR/ha_db.db"
VENV="$SCRIPT_DIR/.venv"

# ── Credentials ───────────────────────────────────────────────────────────────
CREDS_FILE="$SCRIPT_DIR/../v2h_homeassistant/.deploy_credentials"
if [[ ! -f "$CREDS_FILE" ]]; then
    echo "Error: credentials file not found at $CREDS_FILE"
    echo "Copy .deploy_credentials.example to .deploy_credentials and fill in SMB_USER / SMB_PASS."
    exit 1
fi
source "$CREDS_FILE"

# ── Venv (needed for the Python backup snippet) ───────────────────────────────
if [[ ! -x "$VENV/bin/python" ]]; then
    echo "Creating virtual environment..."
    python3 -m venv "$VENV"
fi

# ── Mount ─────────────────────────────────────────────────────────────────────
cleanup() {
    if mountpoint -q "$MOUNT_POINT" 2>/dev/null; then
        sudo umount "$MOUNT_POINT"
        echo "Unmounted $MOUNT_POINT"
    fi
}
trap cleanup EXIT

sudo mkdir -p "$MOUNT_POINT"
echo "Mounting $SHARE..."
sudo mount -t cifs "$SHARE" "$MOUNT_POINT" \
    -o "username=$SMB_USER,password=$SMB_PASS,uid=$(id -u),gid=$(id -g),file_mode=0644,dir_mode=0755"

# ── SQLite online backup (safe while HA is running) ───────────────────────────
echo "Backing up HA database..."
"$VENV/bin/python" - <<EOF
import sqlite3, sys
try:
    src = sqlite3.connect("$HA_DB_SRC")
    dst = sqlite3.connect("$HA_DB_DST")
    src.backup(dst, pages=256)
    dst.close()
    src.close()
except Exception as e:
    print(f"Backup failed: {e}", file=sys.stderr)
    sys.exit(1)
EOF
echo "  Done: $(du -h "$HA_DB_DST" | cut -f1) → $HA_DB_DST"
