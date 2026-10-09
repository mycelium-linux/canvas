#!/bin/sh
set -eu
cd "$(dirname "$0")"
xcrun swift apple/GenerateArtwork.swift apple/Assets.xcassets
if [ "${1:-}" = '--unsigned' ]; then
  shift
  set -- CODE_SIGNING_ALLOWED=NO "$@"
fi
xcodebuild -project apple/CanvasTV.xcodeproj -scheme CanvasTV \
  -configuration Release -destination 'generic/platform=tvOS' \
  -derivedDataPath build/device-derived -archivePath build/CanvasTV.xcarchive \
  -jobs 1 archive "$@"
# No allowProvisioningUpdates, account changes, exports or upload here.
