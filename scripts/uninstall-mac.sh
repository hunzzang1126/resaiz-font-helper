#!/usr/bin/env bash
# Remove resaiz Font Helper from macOS.
set -uo pipefail
launchctl bootout "gui/$(id -u)/com.resaiz.fonthelper" 2>/dev/null || true
rm -f "$HOME/Library/LaunchAgents/com.resaiz.fonthelper.plist"
rm -rf "$HOME/Library/Application Support/resaiz"
echo "resaiz Font Helper removed"
