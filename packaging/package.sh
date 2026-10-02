#!/usr/bin/env bash
# Builds the release archive and AppImage into dist/ from an already built
# portable binary (EMBYCLIENTPLUS_PORTABLE=1 cargo build --release) and the
# libmpv from scripts/build-libmpv.sh. Used by CI and for local checks.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
APP_ID="io.github.krakerz.EmbyClientPlus"
BIN="target/release/embyclientplus"
LIBMPV="third_party/mpv-prefix/lib/libmpv.so.2"
DIST="dist"
TOOLS="packaging/tools"

[ -x "$BIN" ] || { echo "missing $BIN (build with EMBYCLIENTPLUS_PORTABLE=1)" >&2; exit 1; }
[ -e "$LIBMPV" ] || { echo "missing $LIBMPV (run scripts/build-libmpv.sh)" >&2; exit 1; }
if readelf -d "$BIN" | grep -q "$REPO_ROOT"; then
    echo "$BIN has build-machine paths in its rpath; rebuild with EMBYCLIENTPLUS_PORTABLE=1" >&2
    exit 1
fi

# The VapourSynth scripting library libmpv links against; only a stand-in
# so the app starts without SVP (SVP's own copy is used when present).
fallback_vs="$(readlink -f /usr/lib/libvsscript.so 2>/dev/null || true)"
[ -n "$fallback_vs" ] && [ -e "$fallback_vs" ] || { echo "no libvsscript.so to bundle" >&2; exit 1; }

rm -rf "$DIST"
mkdir -p "$DIST" "$TOOLS"

# Shared layout: bin/ (launcher + binary), lib/ (libmpv + fallback).
stage() {
    local root="$1"
    install -Dm755 "$BIN" "$root/bin/embyclientplus-bin"
    install -Dm755 packaging/embyclientplus.sh "$root/bin/embyclientplus"
    install -Dm755 "$fallback_vs" "$root/lib/vapoursynth-fallback/libvsscript.so"
}

# 1. Plain-binary archive with install/uninstall scripts.
bundle="EmbyClientPlus-$VERSION-x86_64"
archive_root="$DIST/$bundle"
stage "$archive_root"
install -Dm755 "$(readlink -f "$LIBMPV")" "$archive_root/lib/libmpv.so.2"
install -Dm644 "packaging/$APP_ID.desktop" "$archive_root/$APP_ID.desktop"
install -Dm644 "packaging/$APP_ID.svg" "$archive_root/$APP_ID.svg"
install -Dm644 packaging/INSTALL.txt "$archive_root/INSTALL.txt"
install -Dm755 packaging/install.sh "$archive_root/install.sh"
install -Dm755 packaging/uninstall.sh "$archive_root/uninstall.sh"
tar -C "$DIST" -czf "$DIST/embyclientplus-$VERSION-linux-x86_64.tar.gz" "$bundle"

# 2. AppImage, bundling GTK4/libadwaita/FFmpeg so it runs on SteamOS too.
fetch() {
    local name="$1" url="$2"
    if [ ! -x "$TOOLS/$name" ]; then
        curl -fsSL -o "$TOOLS/$name" "$url"
        chmod +x "$TOOLS/$name"
    fi
}
fetch linuxdeploy-x86_64.AppImage \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
# Containers have no FUSE; the tools can unpack themselves instead.
export APPIMAGE_EXTRACT_AND_RUN=1

appdir="$DIST/AppDir"
stage "$appdir/usr"
install -Dm755 "$(readlink -f "$LIBMPV")" "$appdir/usr/lib/libmpv.so.2"

# GTK runtime data the libraries alone don't carry (linuxdeploy's GTK
# plugin expects a gtk-4.0 modules dir that current GTK no longer has):
# compiled GSettings schemas, and the symbolic Adwaita icons the UI uses,
# which SteamOS doesn't ship.
schemas="$appdir/usr/share/glib-2.0/schemas"
mkdir -p "$schemas"
cp /usr/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$schemas/"
glib-compile-schemas "$schemas"
icons="$appdir/usr/share/icons"
mkdir -p "$icons/Adwaita" "$icons/hicolor"
cp /usr/share/icons/Adwaita/index.theme "$icons/Adwaita/"
cp -r /usr/share/icons/Adwaita/symbolic "$icons/Adwaita/"
cp /usr/share/icons/hicolor/index.theme "$icons/hicolor/"

export LDAI_OUTPUT="$DIST/EmbyClientPlus-$VERSION-x86_64.AppImage"
export PATH="$REPO_ROOT/$TOOLS:$PATH"
"$TOOLS/linuxdeploy-x86_64.AppImage" \
    --appdir "$appdir" \
    --executable "$appdir/usr/bin/embyclientplus-bin" \
    --library "$appdir/usr/lib/libmpv.so.2" \
    --desktop-file "packaging/$APP_ID.desktop" \
    --icon-file "packaging/$APP_ID.svg" \
    --custom-apprun packaging/AppRun \
    --exclude-library 'libvsscript*' \
    --exclude-library 'libvapoursynth*' \
    --output appimage
rm -rf "$appdir" "$archive_root"
ls -l "$DIST"
