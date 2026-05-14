#!/bin/bash
set -e

PROJECT_DIR="$HOME/v2h_webui/svelte-skeleton-ui"

echo "========================================"
echo "🚀 Starting Build for V2H Web UI"
echo "Project: $PROJECT_DIR"
echo "========================================"

cd "$PROJECT_DIR"

echo "→ Running npm build..."
npm run build

echo "✅ Build completed successfully!"

echo "Build output:"
ls -lh build/

echo "========================================"
echo "Build done at $(date)"
