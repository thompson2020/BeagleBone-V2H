#!/bin/bash
set -e

TARGET="arm-unknown-linux-musleabihf"
BINARY="indra_beaglebone"
PROJECT_DIR="$HOME/BeagleBone-V2H"          

echo "========================================"
echo "🚀 Starting Build for BeagleBone"
echo "Target : $TARGET"
echo "Project: $PROJECT_DIR"
echo "========================================"

cd "$PROJECT_DIR"

echo "→ Running cargo zigbuild..."
cargo zigbuild --target $TARGET --release

echo "✅ Build completed successfully!"

echo "Binary location:"
ls -lh "target/$TARGET/release/$BINARY"

echo "Binary size: $(du -h "target/$TARGET/release/$BINARY" | cut -f1)"

echo "========================================"
echo "Build done at $(date)"
