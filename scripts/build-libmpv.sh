#!/usr/bin/env bash
# Builds a vapoursynth-enabled libmpv (plus a standalone mpv binary) into a
# private prefix inside the repo. Distro mpv builds (CachyOS included) ship
# without vapoursynth, which SVP needs. Nothing is installed outside the repo.
set -euo pipefail

MPV_VERSION="${MPV_VERSION:-v0.41.0}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
THIRD_PARTY="$REPO_ROOT/third_party"
SRC_DIR="$THIRD_PARTY/mpv-src"
BUILD_DIR="$THIRD_PARTY/mpv-build"
PREFIX="${MPV_PREFIX:-$THIRD_PARTY/mpv-prefix}"
SHIM_PC_DIR="$THIRD_PARTY/pkgconfig"

for tool in git meson ninja pkg-config; do
    command -v "$tool" >/dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done
mkdir -p "$THIRD_PARTY" "$SHIM_PC_DIR"

# Pip-era VapourSynth (R74+) ships its headers inside the Python package, a
# vapoursynth.pc with no Libs and a prefix that resolves to the wrong place,
# and no vapoursynth-script.pc at all — while mpv v0.41.0's meson requires
# both .pc files. So locate the real headers/libraries and write our own pair
# of .pc files, shadowing the system ones for this build only.
vs_header="$(find /usr/include /usr/lib -name VSScript4.h -print -quit 2>/dev/null)"
[ -n "$vs_header" ] || { echo "VSScript4.h not found — is vapoursynth installed?" >&2; exit 1; }
vs_includedir="$(dirname "$vs_header")"
vs_libdir=""
for dir in /usr/lib /usr/lib64 /usr/local/lib; do
    if [ -e "$dir/libvapoursynth-script.so" ] && [ -e "$dir/libvapoursynth.so" ]; then
        vs_libdir="$dir"
        break
    fi
done
[ -n "$vs_libdir" ] || { echo "libvapoursynth{,-script}.so not found" >&2; exit 1; }
vs_version="$(pkg-config --modversion vapoursynth 2>/dev/null || echo 80)"

cat > "$SHIM_PC_DIR/vapoursynth.pc" <<EOF
libdir=$vs_libdir
includedir=$vs_includedir

Name: vapoursynth
Description: VapourSynth core (shim for mpv's build)
Version: $vs_version
Libs: -L\${libdir} -lvapoursynth
Cflags: -I\${includedir}
EOF
cat > "$SHIM_PC_DIR/vapoursynth-script.pc" <<EOF
libdir=$vs_libdir

Name: vapoursynth-script
Description: VapourSynth scripting library (shim for mpv's build)
Version: $vs_version
Requires: vapoursynth
Libs: -L\${libdir} -lvapoursynth-script
EOF
export PKG_CONFIG_PATH="$SHIM_PC_DIR${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"

if [ ! -d "$SRC_DIR/.git" ]; then
    git clone --depth 1 --branch "$MPV_VERSION" https://github.com/mpv-player/mpv.git "$SRC_DIR"
fi

if [ ! -f "$BUILD_DIR/build.ninja" ]; then
    meson setup "$BUILD_DIR" "$SRC_DIR" \
        --prefix="$PREFIX" \
        --buildtype=release \
        -Dlibmpv=true \
        -Dcplayer=true \
        -Dvapoursynth=enabled \
        -Dvulkan=enabled \
        -Dwayland=enabled \
        -Degl=enabled \
        -Degl-wayland=enabled \
        -Dgl=enabled \
        -Dplain-gl=enabled
fi

ninja -C "$BUILD_DIR"
ninja -C "$BUILD_DIR" install

# Captured first: piping into `grep -q` SIGPIPEs mpv, which pipefail treats as failure.
vf_list="$("$PREFIX/bin/mpv" --vf=help 2>/dev/null)"
if ! grep -q vapoursynth <<<"$vf_list"; then
    echo "build finished but the vapoursynth filter is missing" >&2
    exit 1
fi
echo "libmpv with vapoursynth installed to $PREFIX"

# SVP's svpflow plugins use VapourSynth API 3, which system VapourSynth R74+
# dropped — so at runtime our libmpv must load SVP's bundled R73 instead.
# libmpv links against the soname libvsscript.so (the system stub's), so
# expose SVP's library under that name; build.rs puts this dir on the rpath.
SVP_DIR="${SVP_DIR:-$HOME/SVP4}"
SVP_VS_DIR="$THIRD_PARTY/svp-vapoursynth"
if [ -e "$SVP_DIR/mpv/libvapoursynth-script.so.0" ]; then
    mkdir -p "$SVP_VS_DIR"
    ln -sfn "$SVP_DIR/mpv/libvapoursynth-script.so.0" "$SVP_VS_DIR/libvsscript.so"
    ln -sfn "$SVP_DIR/mpv/libvapoursynth.so" "$SVP_VS_DIR/libvapoursynth.so"
    echo "linked SVP's VapourSynth from $SVP_DIR into $SVP_VS_DIR"
else
    echo "warning: SVP not found at $SVP_DIR — SVP interpolation won't work" >&2
fi
