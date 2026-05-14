#!/bin/bash
set -e

PROJECT_DIR="$HOME/v2h_webui/svelte-skeleton-ui"

echo "========================================"
echo "🔧 Starting V2H Web UI Dev Server"
echo "Project: $PROJECT_DIR"
echo "========================================"

cd "$PROJECT_DIR"

echo "→ Starting dev server with hot reload..."
echo "→ Access at http://$(hostname -I | awk '{print $1}'):5173"
echo "========================================"

npm run dev -- --host
