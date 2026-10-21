#!/usr/bin/env bash
# Builds the compat AppImage: the same binary and libmpv as the main one,
# packed with quick-sharun/sharun, which bundles glibc and the dynamic loader
# (so it runs on hosts older than Arch, e.g. Debian 11/12, Ubuntu 20.04-22.04)
# and takes the GPU drivers from the host (USE_HOST_DRIVERS_EXPERIMENTAL).
#
# Needs what package.sh needs plus, on the build machine: xvfb-run (+ xauth)
# and strace (quick-sharun runs the app once to find dlopen'ed libraries),
# nss-mdns (libnss_mdns4_minimal for *.local servers), patchelf, glib2 and
# the Adwaita/hicolor icon themes. Run from a root shell (a container): it
# puts libmpv into /usr/lib, where quick-sharun expects every library.
#
# Writes dist/EmbyClientPlus-<version>-x86_64-compat.AppImage. The name must
# not end in "-x86_64.AppImage": the in-app updater picks the main build by
# that suffix. CI runs this as its own step, and a failure only means the
# release goes out without the compat file.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
APP_ID="io.github.krakerz.EmbyClientPlus"
BIN="${BIN:-target/release/embyclientplus}"
LIBMPV="third_party/mpv-prefix/lib/libmpv.so.2"
DIST="${DIST:-dist}"
TOOLS="packaging/tools"
OUT="$DIST/EmbyClientPlus-$VERSION-x86_64-compat.AppImage"
# What the app plays while quick-sharun records the libraries it loads
# (needs the app's `--player`); one variable so it is easy to change.
TRACE_URL="${COMPAT_TRACE_URL:-av://lavfi:testsrc2=duration=5}"

# shellcheck source=packaging/appimage/quick-sharun.pin
. packaging/appimage/quick-sharun.pin

[ -x "$BIN" ] || { echo "missing $BIN (build with EMBYCLIENTPLUS_PORTABLE=1)" >&2; exit 1; }
[ -e "$LIBMPV" ] || { echo "missing $LIBMPV (run scripts/build-libmpv.sh)" >&2; exit 1; }
for tool in patchelf readelf xvfb-run strace curl sha256sum glib-compile-schemas; do
    command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done
[ "$(id -u)" = 0 ] || { echo "run as root (in a container): libmpv goes into /usr/lib" >&2; exit 1; }

mkdir -p "$DIST" "$TOOLS"
work="$(mktemp -d "$DIST/.compat-work.XXXXXX")"
trap 'rm -rf "$work"' EXIT
rm -f "$OUT"

# 1. The pinned quick-sharun, checked against its sha256.
quick_sharun="$TOOLS/quick-sharun-${QUICK_SHARUN_COMMIT:0:12}.sh"
if [ ! -x "$quick_sharun" ]; then
    curl -fsSL -o "$quick_sharun.part" \
        "https://raw.githubusercontent.com/pkgforge-dev/Anylinux-AppImages/$QUICK_SHARUN_COMMIT/useful-tools/quick-sharun.sh"
    chmod +x "$quick_sharun.part"
    mv "$quick_sharun.part" "$quick_sharun"
fi
echo "$QUICK_SHARUN_SHA256  $quick_sharun" | sha256sum -c - || {
    rm -f "$quick_sharun"
    echo "quick-sharun does not match packaging/appimage/quick-sharun.pin" >&2
    exit 1
}

# 2. The binary, without an RPATH. The portable build's $ORIGIN/../lib
# (DT_RPATH, not RUNPATH) would beat sharun's --library-path and the
# library path the SVP hook sets, so the bundled glibc could end up mixed
# with libraries picked from a path nobody meant.
stage="$work/stage"
mkdir -p "$stage"
install -Dm755 "$BIN" "$stage/embyclientplus"
patchelf --remove-rpath "$stage/embyclientplus"
if readelf -d "$stage/embyclientplus" | grep -E 'RPATH|RUNPATH'; then
    echo "the binary still has an RPATH/RUNPATH" >&2
    exit 1
fi

# libmpv where quick-sharun finds it like any system library.
install -Dm755 "$(readlink -f "$LIBMPV")" /usr/lib/libmpv.so.2

# 3. Deploy: GTK4/libadwaita/FFmpeg and the rest come from ldd and from
# running the app once under xvfb (STRACE_*), pipewire/pulse from the flags.
export APPDIR="$work/AppDir"
export OUTPATH="$work/out"
export ARCH=x86_64 VERSION
export DESKTOP="$REPO_ROOT/packaging/$APP_ID.desktop"
export ICON="$REPO_ROOT/packaging/$APP_ID.svg"
export USE_HOST_DRIVERS_EXPERIMENTAL=1
export DEPLOY_PIPEWIRE=1 DEPLOY_PULSE=1
export STRACE_BINARY=embyclientplus
export STRACE_FLAGS="--player $TRACE_URL"
export STRACE_TIME="${COMPAT_STRACE_TIME:-12}"
# No guessed zsync update info: the app updates itself from the release.
unset GITHUB_REPOSITORY UPINFO

"$quick_sharun" "$stage/embyclientplus" \
    /usr/lib/libmpv.so.2 \
    /usr/lib/libnss_mdns4_minimal.so.2

# 4. What quick-sharun doesn't know about: GTK4's GSettings schemas, the
# symbolic Adwaita icons the UI uses (hosts may not have them), the SVP
# hook, and the flavour for the app.
schemas="$APPDIR/share/glib-2.0/schemas"
mkdir -p "$schemas"
cp /usr/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$schemas/"
glib-compile-schemas "$schemas"
icons="$APPDIR/share/icons"
mkdir -p "$icons/Adwaita" "$icons/hicolor"
cp /usr/share/icons/Adwaita/index.theme "$icons/Adwaita/"
cp -r /usr/share/icons/Adwaita/symbolic "$icons/Adwaita/"
cp /usr/share/icons/hicolor/index.theme "$icons/hicolor/"

install -Dm644 packaging/svp-detect.sh "$APPDIR/share/embyclientplus/svp-detect.sh"
install -Dm644 packaging/appimage/svp.src.hook "$APPDIR/bin/05-svp.hook"
echo 'EMBYCLIENTPLUS_FLAVOUR=compat' >> "$APPDIR/.env"

# 5. Pack.
"$quick_sharun" --make-appimage
built=("$OUTPATH"/*.AppImage)
[ -f "${built[0]}" ] && [ "${#built[@]}" = 1 ] || { echo "expected one AppImage in $OUTPATH" >&2; exit 1; }
mv "${built[0]}" "$OUT"
chmod +x "$OUT"
echo "built $OUT ($(du -h "$OUT" | cut -f1))"
