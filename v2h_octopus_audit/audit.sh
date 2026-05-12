#!/bin/bash
# Run the Octopus slot audit against a local ha_db.db.
# Run fetch_db.sh first if you need a fresh copy of the database.
#
# Prerequisites:  python3-venv  (sudo apt install python3-venv)
# Config:         copy config.example.toml → config.toml and set your API key

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VENV="$SCRIPT_DIR/.venv"

# ── Config check ──────────────────────────────────────────────────────────────
if [[ ! -f "$SCRIPT_DIR/config.toml" ]]; then
    echo "Error: config.toml not found."
    echo "Copy config.example.toml → config.toml and set your Octopus API key."
    exit 1
fi

# ── Venv setup ────────────────────────────────────────────────────────────────
if [[ ! -x "$VENV/bin/python" ]]; then
    echo "Creating virtual environment..."
    python3 -m venv "$VENV"
fi

if ! "$VENV/bin/pip" show requests &>/dev/null; then
    echo "Installing dependencies..."
    "$VENV/bin/pip" install --quiet -r "$SCRIPT_DIR/requirements.txt"
fi

# ── Run ───────────────────────────────────────────────────────────────────────
cd "$SCRIPT_DIR"
"$VENV/bin/python" audit.py
