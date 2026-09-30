#!/bin/bash
# Deploy Canvas to home-pi.local
# Usage: ./scripts/deploy-home-pi.sh

set -e

TARGET="ajm@home-pi.local"
CANVAS_BIN="./target/debug/canvas"
MPD_CONFIG="./config/mpd.conf"
LYRICS_DIR="./config/lyrics"

echo "Building Canvas for ARM64..."
# Cross-compile for ARM64 (assuming agora-one is an ARM64 Linux box)
cargo build --release --package canvas --target aarch64-unknown-linux-gnu

echo "Creating deployment package..."
mkdir -p ./deploy-home-pi
cp ./target/aarch64-unknown-linux-gnu/release/canvas ./deploy-home-pi/
cp -r ./config ./deploy-home-pi/

if [ -d "./lyrics" ]; then
    cp -r ./lyrics ./deploy-home-pi/
fi

echo "Deploying to $TARGET..."
scp -r ./deploy-home-pi/* ${TARGET}:/opt/canvas/

echo "Setting up MPD service..."
ssh ${TARGET} << 'ENDSSH'
sudo systemctl stop mpd 2>/dev/null || true
sudo mkdir -p /var/lib/mpd/music /var/lib/mpd/cache /var/lib/mpd/playlists
sudo mkdir -p /opt/canvas/lyrics
sudo chown -R mpd:mpd /var/lib/mpd
sudo systemctl start mpd
ENDSSH

echo "Deployment complete!"
echo "Next steps:"
echo "1. Copy MPD credentials to /opt/canvas/config/mpd.conf"
echo "2. Place .lrc lyric files in /opt/canvas/lyrics/"
echo "3. Launch Canvas on home-pi.local"
