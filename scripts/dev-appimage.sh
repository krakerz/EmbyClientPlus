#!/usr/bin/env bash
# Builds a test AppImage locally, the way CI does, and copies it to the
# repo root as EmbyClientPlus-<version>-dev-x86_64.AppImage (gitignored) for
# copying to another machine, e.g. a Steam Deck, without waiting for a
# GitHub release. The portable binary builds in its own target dir, so the
# normal `cargo build --release` isn't relinked each time.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
TARGET_DIR="target/portable"
OUT="EmbyClientPlus-$VERSION-dev-x86_64.AppImage"

[ -e third_party/mpv-prefix/lib/libmpv.so.2 ] || scripts/build-libmpv.sh
EMBYCLIENTPLUS_PORTABLE=1 CARGO_TARGET_DIR="$TARGET_DIR" cargo build --release --locked
BIN="$TARGET_DIR/release/embyclientplus" packaging/package.sh

rm -f EmbyClientPlus-*-dev-x86_64.AppImage
cp dist/*-x86_64.AppImage "$OUT"
chmod +x "$OUT"
echo "built $OUT ($(du -h "$OUT" | cut -f1)), from $(git rev-parse --short HEAD)$(git diff --quiet || echo ' + uncommitted changes')"
