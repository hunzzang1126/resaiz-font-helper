#!/usr/bin/env bash
# resaiz Font Helper: install on macOS and start it at login.
# Usage: bash install-mac.sh [path-to-resaiz-font-helper-binary]
# Without an argument the script looks for the binary next to itself.
set -euo pipefail

SRC="${1:-$(dirname "$0")/resaiz-font-helper}"
if [ ! -f "$SRC" ]; then
  echo "binary not found: $SRC" >&2
  exit 1
fi
DEST_DIR="$HOME/Library/Application Support/resaiz"
DEST="$DEST_DIR/resaiz-font-helper"
PLIST="$HOME/Library/LaunchAgents/com.resaiz.fonthelper.plist"
LOG="$DEST_DIR/font-helper.log"

mkdir -p "$DEST_DIR" "$HOME/Library/LaunchAgents"
cp "$SRC" "$DEST"
chmod +x "$DEST"
# The binary is not notarized yet; drop the quarantine flag Gatekeeper set on download.
xattr -d com.apple.quarantine "$DEST" 2>/dev/null || true

launchctl bootout "gui/$(id -u)/com.resaiz.fonthelper" 2>/dev/null || true
cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.resaiz.fonthelper</string>
  <key>ProgramArguments</key><array><string>$DEST</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$LOG</string>
  <key>StandardErrorPath</key><string>$LOG</string>
</dict>
</plist>
PLIST
launchctl bootstrap "gui/$(id -u)" "$PLIST"
sleep 1
if curl -fsS http://127.0.0.1:57731/v1/health >/dev/null 2>&1; then
  echo "resaiz Font Helper is running: $(curl -fsS http://127.0.0.1:57731/v1/health)"
else
  echo "installed; the helper starts within a few seconds (log: $LOG)"
fi
echo "It starts automatically at login. To remove: bash uninstall-mac.sh"
