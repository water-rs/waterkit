#!/bin/bash
# Build, bundle, and run the notification test as a macOS app bundle.
# This allows UserNotifications framework to work with action buttons.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/../../.."

# Build the binary
cargo build -p waterkit-notification-test

# Create .app bundle
APP_DIR="target/debug/NotificationTest.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR"

# Copy binary
cp target/debug/waterkit-notification-test "$MACOS_DIR/waterkit-notification-test"

# Copy Info.plist
cp tests/macos/notification/Info.plist "$CONTENTS_DIR/Info.plist"

# Ad-hoc sign the app bundle
codesign --force --sign - "$APP_DIR"

LOG_FILE="target/debug/notification-test.log"

# Clear old log
> "$LOG_FILE"

echo "Launching NotificationTest.app..."
echo "(A notification with action buttons should appear)"
echo ""

# Launch via open command
open -W "$APP_DIR"

echo "=== Test Output ==="
cat "$LOG_FILE" 2>/dev/null || echo "(No output captured)"
