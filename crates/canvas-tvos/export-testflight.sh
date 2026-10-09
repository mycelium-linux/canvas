#!/bin/sh
set -eu
cd "$(dirname "$0")"
# Requires an appropriately signed archive/profile. Exports locally, not upload.
xcodebuild -exportArchive -archivePath build/CanvasTV.xcarchive \
  -exportOptionsPlist apple/ExportOptions.plist -exportPath build/testflight "$@"
