#!/usr/bin/env bash
# Runs the compat AppImage on old distros, in docker (nothing is installed on
# the host). For every image: install xvfb and Mesa's software rasterizer
# (llvmpipe) in a throwaway container, then
#   1. `--version` must print the Cargo.toml version and "(compat)",
#   2. the app's headless smoke test (EMBYCLIENTPLUS_SMOKE_TEST=1 --player
#      <url> under xvfb) must print `smoke:` lines and exit 0,
#   3. nothing may print `GLIBC_`, `cannot open shared object` or `CRITICAL`.
# Exits non-zero when any image failed.
#
#   scripts/test-appimage-compat.sh <AppImage> [image...]
# Default images: debian:11 debian:12 ubuntu:20.04 ubuntu:22.04 ubuntu:24.04.
# Logs go to $LOG_DIR (default: a new temp dir, printed at the end).
# The smoke test's input is $SMOKE_URL.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SMOKE_URL="${SMOKE_URL:-av://lavfi:testsrc2=duration=5}"
DEFAULT_IMAGES=(debian:11 debian:12 ubuntu:20.04 ubuntu:22.04 ubuntu:24.04)

[ $# -ge 1 ] || { echo "usage: $0 <AppImage> [image...]" >&2; exit 2; }
appimage="$(readlink -f "$1")"
shift
[ -f "$appimage" ] || { echo "no such file: $appimage" >&2; exit 2; }
command -v docker >/dev/null || { echo "needs docker" >&2; exit 2; }
images=("$@")
[ ${#images[@]} -gt 0 ] || images=("${DEFAULT_IMAGES[@]}")

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -n 1)"
log_dir="${LOG_DIR:-$(mktemp -d)}"
mkdir -p "$log_dir"

# Runs inside the container (as root). Prints "RESULT: ..." lines; the host
# side turns FAIL lines into the exit status.
inner() {
    cat <<'INNER'
set -u
export DEBIAN_FRONTEND=noninteractive
# Debian 11 is EOL: its repos moved to archive.debian.org (no -updates there).
if grep -q '^VERSION_CODENAME=bullseye' /etc/os-release 2>/dev/null; then
    rm -f /etc/apt/sources.list.d/*
    cat >/etc/apt/sources.list <<'EOF'
deb http://archive.debian.org/debian bullseye main
deb http://archive.debian.org/debian-security bullseye-security main
EOF
    echo 'Acquire::Check-Valid-Until "false";' >/etc/apt/apt.conf.d/99archive
fi
apt-get update -qq >/dev/null
# libgl1-mesa-dri has llvmpipe; the others are what a desktop has anyway.
# libglx-mesa0/libegl-mesa0 don't exist on older releases, so they are not
# listed (libgl1 / libegl1 pull in what they need).
apt-get install -y -qq --no-install-recommends xvfb xauth ca-certificates \
    libgl1-mesa-dri libegl1 libgl1 mesa-vulkan-drivers \
    libx11-6 libxcb1 fonts-dejavu-core >/tmp/apt.log 2>&1 \
    || { tail -n 5 /tmp/apt.log; echo "RESULT: FAIL: could not install xvfb/mesa"; exit 0; }
. /etc/os-release
echo "== $PRETTY_NAME, glibc $(ldd --version | head -n 1 | awk '{print $NF}')"

export APPIMAGE_EXTRACT_AND_RUN=1
export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
cp /test/app.AppImage /tmp/app.AppImage; chmod +x /tmp/app.AppImage
app=/tmp/app.AppImage

echo "== --version"
out="$(cd /tmp && timeout 120 "$app" --version 2>&1)"; code=$?
echo "$out"; all="$out"
if [ $code -ne 0 ]; then echo "RESULT: FAIL: --version exited $code"
elif ! grep -qF "$EXPECT_VERSION" <<<"$out"; then echo "RESULT: FAIL: --version lacks $EXPECT_VERSION"
elif ! grep -qF '(compat)' <<<"$out"; then echo "RESULT: FAIL: --version has no '(compat)'"
else echo "RESULT: ok: --version"; fi

echo "== smoke test ($SMOKE_URL)"
# Xvfb is started by hand: xvfb-run can report 0 when the app was killed by a
# signal (a segfault looked like a pass with no output).
[ -s /etc/machine-id ] || tr -d - </proc/sys/kernel/random/uuid >/etc/machine-id
Xvfb :77 -screen 0 1280x720x24 >/tmp/xvfb.log 2>&1 &
xvfb_pid=$!
sleep 2
out="$(cd /tmp && DISPLAY=:77 GTK_A11Y=none LIBGL_ALWAYS_SOFTWARE=1 EMBYCLIENTPLUS_SMOKE_TEST=1 \
    timeout 90 "$app" --player "$SMOKE_URL" 2>&1)"; code=$?
kill "$xvfb_pid" 2>/dev/null
[ $code -lt 124 ] || echo "(smoke test exit code $code: 124 = timeout, 128+n = signal n)"
echo "$out"; all="$all
$out"
if ! grep -q '^smoke:' <<<"$out"; then echo "RESULT: FAIL: no 'smoke:' lines (exit $code)"
elif [ $code -ne 0 ]; then echo "RESULT: FAIL: smoke test exited $code"
else echo "RESULT: ok: smoke test"; fi

echo "== scan"
bad=0
for pattern in 'GLIBC_' 'cannot open shared object' 'CRITICAL'; do
    if grep -n -- "$pattern" <<<"$all"; then
        echo "RESULT: FAIL: output contains '$pattern'"; bad=1
    fi
done
[ $bad -eq 0 ] && echo "RESULT: ok: no GLIBC_/missing library/CRITICAL in the output"
exit 0
INNER
}

status=0
summary=()
for image in "${images[@]}"; do
    log="$log_dir/${image//[:\/]/_}.log"
    echo ">>> $image (log: $log)"
    docker run --rm \
        -v "$appimage":/test/app.AppImage:ro \
        -e EXPECT_VERSION="$version" -e SMOKE_URL="$SMOKE_URL" \
        "$image" bash -c "$(inner)" </dev/null >"$log" 2>&1
    docker_code=$?
    grep -E '^(== |RESULT:)' "$log" | sed 's/^/    /'
    if [ $docker_code -ne 0 ]; then
        echo "    RESULT: FAIL: container exited $docker_code"
        summary+=("FAIL  $image (docker exit $docker_code)")
        status=1
    elif grep -q '^RESULT: FAIL' "$log"; then
        summary+=("FAIL  $image")
        status=1
    else
        summary+=("pass  $image$(grep -q '^RESULT: WARN' "$log" && echo ' (with warnings)')")
    fi
done

echo
printf '%s\n' "${summary[@]}"
echo "logs: $log_dir"
exit $status
