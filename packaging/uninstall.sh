#!/usr/bin/env bash
# Removes what install.sh installed. Settings, login and logs in
# ~/.config/embyclientplus are kept, so a reinstall picks up where you left.
set -euo pipefail

app_id="io.github.krakerz.EmbyClientPlus"
root="$HOME/.local/share/embyclientplus"
apps_dir="$HOME/.local/share/applications"

rm -rf "$root"
rm -f "$HOME/.local/bin/embyclientplus"
rm -f "$apps_dir/$app_id.desktop"
rm -f "$HOME/.local/share/icons/hicolor/scalable/apps/$app_id.svg"
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/embyclientplus"

command -v update-desktop-database >/dev/null 2>&1 &&
    update-desktop-database "$apps_dir" 2>/dev/null || true

echo "EmbyClientPlus removed."
echo "Your settings are still in ${XDG_CONFIG_HOME:-$HOME/.config}/embyclientplus (delete it to start fresh)."
