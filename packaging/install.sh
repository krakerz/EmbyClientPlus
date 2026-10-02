#!/usr/bin/env bash
# Installs EmbyClientPlus for the current user. Run from inside the
# extracted release archive, next to this script:
#   ./install.sh
# Safe to re-run (reinstalls / upgrades in place).
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app_id="io.github.krakerz.EmbyClientPlus"
root="$HOME/.local/share/embyclientplus"
bin_dir="$HOME/.local/bin"
apps_dir="$HOME/.local/share/applications"
icon_dir="$HOME/.local/share/icons/hicolor/scalable/apps"

if [ ! -x "$dir/bin/embyclientplus-bin" ]; then
    echo "error: bin/embyclientplus-bin not found next to this script ($dir)" >&2
    exit 1
fi

mkdir -p "$root" "$bin_dir" "$apps_dir" "$icon_dir"
# Replace bin/ and lib/ wholesale so no stale files from older versions stay.
rm -rf "$root/bin" "$root/lib"
cp -a "$dir/bin" "$dir/lib" "$root/"
ln -sfn "$root/bin/embyclientplus" "$bin_dir/embyclientplus"
install -m 644 "$dir/$app_id.svg" "$icon_dir/$app_id.svg"

# Absolute Exec/Icon: launchers (KDE's menu, Steam) don't reliably inherit
# a $PATH that includes ~/.local/bin.
sed \
    -e "s|^Exec=.*|Exec=$root/bin/embyclientplus|" \
    -e "s|^Icon=.*|Icon=$icon_dir/$app_id.svg|" \
    "$dir/$app_id.desktop" > "$apps_dir/$app_id.desktop"
chmod 644 "$apps_dir/$app_id.desktop"

command -v update-desktop-database >/dev/null 2>&1 &&
    update-desktop-database "$apps_dir" 2>/dev/null || true

echo "Installed to $root"
case ":$PATH:" in
*":$bin_dir:"*) ;;
*) echo "Note: $bin_dir isn't on your \$PATH; the menu entry works regardless." ;;
esac
echo "Run with: embyclientplus  (or from your applications menu)"
echo "Steam Game Mode: add $root/bin/embyclientplus as a Non-Steam Game."
echo "Updates: Preferences > Updates, or a notice at start-up."
