#!/usr/bin/env bash
# Builds the release archive and AppImage into dist/ from an already built
# portable binary (EMBYCLIENTPLUS_PORTABLE=1 cargo build --release) and the
# libmpv from scripts/build-libmpv.sh. Used by CI and for local checks.
# With COMPAT_APPIMAGE=1 it also builds the compat AppImage at the end
# (packaging/appimage/make-compat-appimage.sh; CI runs that as a step of its
# own, so a failure there leaves the rest of the release intact).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
APP_ID="io.github.krakerz.EmbyClientPlus"
# Overridable for builds in another target dir (scripts/dev-appimage.sh).
BIN="${BIN:-target/release/embyclientplus}"
LIBMPV="third_party/mpv-prefix/lib/libmpv.so.2"
DIST="${DIST:-dist}"
TOOLS="packaging/tools"

[ -x "$BIN" ] || { echo "missing $BIN (build with EMBYCLIENTPLUS_PORTABLE=1)" >&2; exit 1; }
[ -e "$LIBMPV" ] || { echo "missing $LIBMPV (run scripts/build-libmpv.sh)" >&2; exit 1; }
if readelf -d "$BIN" | grep -q "$REPO_ROOT"; then
    echo "$BIN has build-machine paths in its rpath; rebuild with EMBYCLIENTPLUS_PORTABLE=1" >&2
    exit 1
fi

# libmpv links libvsscript.so: our stand-in (packaging/vsshim), which loads
# SVP's VapourSynth or the system's at run time, as the app tells it.
vsshim_dir="$(mktemp -d)"
cc -shared -fPIC -O2 -Wall -Wl,-soname,libvsscript.so -o "$vsshim_dir/libvsscript.so" \
    packaging/vsshim/vsshim.c -ldl

rm -rf "$DIST"
mkdir -p "$DIST" "$TOOLS"

# Shared layout: bin/ (launcher + binary), lib/ (libmpv + VapourSynth stand-in).
stage() {
    local root="$1"
    install -Dm755 "$BIN" "$root/bin/embyclientplus-bin"
    install -Dm755 packaging/embyclientplus.sh "$root/bin/embyclientplus"
    install -Dm755 "$vsshim_dir/libvsscript.so" "$root/lib/vsshim/libvsscript.so"
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
cp -r packaging/steam-library "$archive_root/steam-library"
tar -C "$DIST" -czf "$DIST/embyclientplus-$VERSION-linux-x86_64.tar.gz" "$bundle"

# The main AppImage runs on the host's glibc: report when any of its ELF files
# needs a newer one than MAX_GLIBC (2.41 by default). A warning unless
# STRICT_GLIBC=1, since Arch's own libraries move past it (2.44 in 2026-10)
# and failing would stop every Linux release; hosts older than that use the
# compat AppImage, which brings its own glibc.
check_glibc() {
    local root="$1" max="${MAX_GLIBC:-2.41}" bad=0 file need
    while IFS= read -r -d '' file; do
        [ "$(head -c 4 "$file" 2>/dev/null)" = $'\177ELF' ] || continue
        need="$(objdump -T "$file" 2>/dev/null | grep -o 'GLIBC_[0-9][0-9.]*' |
            sed 's/^GLIBC_//' | sort -Vu | tail -n 1 || true)"
        [ -n "$need" ] || continue
        if [ "$(printf '%s\n%s\n' "$need" "$max" | sort -V | tail -n 1)" != "$max" ]; then
            echo "glibc guard: ${file#"$root"/} needs GLIBC_$need (> $max)" >&2
            bad=1
        fi
    done < <(find "$root" -type f -print0)
    if [ "$bad" = 1 ]; then
        echo "the AppImage needs glibc newer than $max" >&2
        [ -n "${GITHUB_ACTIONS:-}" ] &&
            echo "::warning::main AppImage needs glibc newer than $max; older hosts need the compat AppImage"
        [ "${STRICT_GLIBC:-0}" = 1 ] && return 1
        return 0
    fi
    echo "glibc guard: every ELF in $root needs glibc <= $max"
}

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
check_glibc "$appdir"
rm -rf "$appdir" "$archive_root"

# 3. Steam library artwork on its own, for AppImage users.
bsdtar -a -cf "$DIST/EmbyClientPlus-$VERSION-steam-library.zip" -C packaging steam-library

# 4. Compat AppImage, for hosts older than the main one's glibc (opt in).
if [ "${COMPAT_APPIMAGE:-0}" = 1 ]; then
    packaging/appimage/make-compat-appimage.sh
fi
ls -l "$DIST"
