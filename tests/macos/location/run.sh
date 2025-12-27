#!/bin/bash
# Build, bundle, and run the location test as a macOS app bundle.
# This allows the system to show the location permission dialog.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/../../.."

# Build the binary
cargo build -p waterkit-location-test

# Create .app bundle
APP_DIR="target/debug/LocationTest.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
LOG_FILE="target/debug/location-test.log"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR"

# Copy binary directly (no wrapper)
cp target/debug/location-test "$MACOS_DIR/location-test"

# Copy Info.plist
cp tests/macos/location/Info.plist "$CONTENTS_DIR/Info.plist"

# Ad-hoc sign the app bundle
codesign --force --sign - "$APP_DIR"

# Clear old log
> "$LOG_FILE"

echo "Launching LocationTest.app..."
echo "(A location permission dialog should appear)"
echo ""

# Launch via open command - this triggers the permission dialog
open -W "$APP_DIR"

echo "=== Test Output ==="
cat "$LOG_FILE" 2>/dev/null || echo "(No output captured)"
