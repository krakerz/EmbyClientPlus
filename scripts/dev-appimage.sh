#!/usr/bin/env bash
# Builds a test AppImage locally, the way CI does, and copies it to the
# repo root as EmbyClientPlus-<version>-dev-x86_64.AppImage (gitignored) for
# copying to another machine, e.g. a Steam Deck, without waiting for a
# GitHub release. The portable binary builds in its own target dir, so the
# normal `cargo build --release` isn't relinked each time.
#
#   scripts/dev-appimage.sh            main AppImage, built on this machine
#   scripts/dev-appimage.sh --compat   main + compat AppImage, built inside
#                                      an archlinux container like CI does
#                                      (nothing is installed on the host)
#
# --compat leaves EmbyClientPlus-<version>-compat-dev-x86_64.AppImage next to
# the main one. Check it on old distros with
# scripts/test-appimage-compat.sh <that file>.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
OUT="EmbyClientPlus-$VERSION-dev-x86_64.AppImage"
OUT_COMPAT="EmbyClientPlus-$VERSION-compat-dev-x86_64.AppImage"

mode=""
case "${1:-}" in
"") ;;
--compat) mode=compat ;;
*) echo "usage: $0 [--compat]" >&2; exit 2 ;;
esac

if [ "$mode" = compat ]; then
    command -v docker >/dev/null || { echo "--compat needs docker" >&2; exit 1; }
    # The container's own third_party (a named volume over the repo's), cargo
    # home, pacman cache and target dir keep it from touching the host's
    # builds and make re-runs fast. Remove the volumes with
    # `docker volume rm ecp-appimage-third-party ecp-appimage-cargo ecp-appimage-pacman`.
    docker run --rm -i \
        -v "$REPO_ROOT":/work \
        -v ecp-appimage-third-party:/work/third_party \
        -v ecp-appimage-cargo:/cargo-home \
        -v ecp-appimage-pacman:/var/cache/pacman/pkg \
        -w /work \
        -e CARGO_HOME=/cargo-home \
        -e CARGO_TARGET_DIR=target/appimage-container \
        -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
        -e MAX_GLIBC -e STRICT_GLIBC \
        archlinux:latest bash -euo pipefail -s <<'CONTAINER'
# Hosted CI's package list (.github/workflows/build.yml), plus what the
# compat AppImage's deployment run needs.
pacman -Syu --noconfirm --needed \
    base-devel git rust meson ninja pkgconf curl file patchelf \
    gtk4 libadwaita ffmpeg libplacebo libass vapoursynth zimg lcms2 uchardet \
    wayland wayland-protocols libxkbcommon libxpresent libxss libxv libxrandr \
    libdrm mesa vulkan-headers vulkan-icd-loader libva libvdpau libdisplay-info \
    pipewire libpulse alsa-lib systemd-libs desktop-file-utils \
    xorg-server-xvfb xorg-xauth strace nss-mdns libarchive
# Files created below belong to the host user again, whatever happens.
trap 'chown -R "$HOST_UID:$HOST_GID" target/appimage-container dist packaging/tools 2>/dev/null || true' EXIT
git config --global --add safe.directory /work
[ -e third_party/mpv-prefix/lib/libmpv.so.2 ] || scripts/build-libmpv.sh
EMBYCLIENTPLUS_PORTABLE=1 cargo build --release --locked
BIN=target/appimage-container/release/embyclientplus COMPAT_APPIMAGE=1 packaging/package.sh
CONTAINER
    rm -f EmbyClientPlus-*-dev-x86_64.AppImage
    cp dist/*-x86_64.AppImage "$OUT"
    cp dist/*-x86_64-compat.AppImage "$OUT_COMPAT"
    chmod +x "$OUT" "$OUT_COMPAT"
    echo "built $OUT ($(du -h "$OUT" | cut -f1)) and $OUT_COMPAT ($(du -h "$OUT_COMPAT" | cut -f1)), from $(git rev-parse --short HEAD)$(git diff --quiet || echo ' + uncommitted changes')"
    exit 0
fi

TARGET_DIR="target/portable"
[ -e third_party/mpv-prefix/lib/libmpv.so.2 ] || scripts/build-libmpv.sh
EMBYCLIENTPLUS_PORTABLE=1 CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release --locked
BIN="$TARGET_DIR/release/embyclientplus" packaging/package.sh

rm -f EmbyClientPlus-*-dev-x86_64.AppImage
cp dist/*-x86_64.AppImage "$OUT"
chmod +x "$OUT"
echo "built $OUT ($(du -h "$OUT" | cut -f1)), from $(git rev-parse --short HEAD)$(git diff --quiet || echo ' + uncommitted changes')"
