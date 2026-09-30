#!/bin/bash
# Cross-compile Canvas for home-pi.local via agora-one
# Usage: ./scripts/build-home-pi.sh

set -e

echo "Building Canvas for home-pi.local (ARM64 Linux)..."

# Build with lockout to avoid thrashing parallel builds
if command -v bd &> /dev/null; then
    echo "Using bd lock..."
    bd lock --timeout 300 || cargo +lockout build
else
    echo "No lock mechanism found, proceeding with build..."
fi

# Cross-compile for ARM64
cargo build --release --target aarch64-unknown-linux-gnu -p canvas

echo ""
echo "Build complete!"
echo "Binary: ./target/aarch64-unknown-linux-gnu/release/canvas"
echo ""
echo "To deploy to home-pi.local:"
echo "  scp ./target/aarch64-unknown-linux-gnu/release/canvas ajm@home-pi.local:/opt/canvas/"
echo ""
