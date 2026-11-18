#!/usr/bin/env bash
# Runs the Linux release packages on a distro in docker (nothing is installed
# on the host): scripts/test-linux-packages.inner.sh per image. Passes when
# every package found either plays the smoke test or, where the distro's glibc
# is too old, stops with the launcher's clear message; the compat AppImage
# must always play.
#
#   scripts/test-linux-packages.sh <dir with the packages> <image>...
# Logs go to $LOG_DIR (default: a new temp dir, printed at the end).
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[ $# -ge 2 ] || { echo "usage: $0 <dir> <image>..." >&2; exit 2; }
src="$(readlink -f "$1")"
shift
log_dir="${LOG_DIR:-$(mktemp -d)}"
mkdir -p "$log_dir"

# The inner script expects fixed names.
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cp "$REPO_ROOT/scripts/test-linux-packages.inner.sh" "$stage/inner.sh"
shopt -s nullglob
for f in "$src"/*-x86_64.AppImage; do ln "$f" "$stage/main.AppImage" 2>/dev/null || cp "$f" "$stage/main.AppImage"; done
for f in "$src"/*-x86_64-compat.AppImage; do ln "$f" "$stage/compat.AppImage" 2>/dev/null || cp "$f" "$stage/compat.AppImage"; done
for f in "$src"/*-linux-x86_64.tar.gz; do ln "$f" "$stage/app.tar.gz" 2>/dev/null || cp "$f" "$stage/app.tar.gz"; done
ls "$stage"/*.AppImage "$stage"/*.tar.gz >/dev/null 2>&1 || { echo "no packages in $src" >&2; exit 2; }

status=0
for image in "$@"; do
    log="$log_dir/${image//[:\/]/_}.log"
    echo ">>> $image (log: $log)"
    docker run --rm -v "$stage:/in:ro" "$image" sh /in/inner.sh </dev/null >"$log" 2>&1
    code=$?
    grep -E '^(== |RESULT:)' "$log" | sed 's/^/    /'
    results="$(grep '^RESULT:' "$log")"
    if [ $code -ne 0 ] || [ -z "$results" ]; then
        echo "    FAIL: container exited $code"
        status=1
    elif grep -v -E 'ok \(plays\)$|glibc-too-old \(clear message\)$' <<<"$results" | grep -q . ||
        grep -q 'compat-AppImage glibc-too-old' <<<"$results"; then
        echo "    FAIL"
        status=1
    fi
done
echo "logs: $log_dir"
exit $status
