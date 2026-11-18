#!/bin/sh
# Runs inside one distro container for scripts/test-linux-packages.sh, with the
# packages in /in (main.AppImage, compat.AppImage, app.tar.gz; missing ones are
# skipped). Both AppImages run with basic desktop libraries only (no system
# GTK), then GTK 4 + libadwaita go in for the tar.gz. Each: --version, then the
# headless playback smoke test under Xvfb. Prints "RESULT: <pkg> <verdict>".
set -u
SMOKE_URL="av://lavfi:testsrc2=duration=5"
pm=""
for c in apt-get dnf zypper pacman; do command -v $c >/dev/null 2>&1 && { pm=$c; break; }; done
install() { # each package on its own; missing names are fine
    case $pm in
    apt-get) [ -f /tmp/apt-updated ] || { apt-get update -qq >/dev/null 2>&1; touch /tmp/apt-updated; }
        for p in "$@"; do DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "$p" >/dev/null 2>&1; done ;;
    dnf) for p in "$@"; do dnf -y -q install --setopt=install_weak_deps=False "$p" >/dev/null 2>&1; done ;;
    zypper) for p in "$@"; do zypper -q -n install --no-recommends "$p" >/dev/null 2>&1; done ;;
    pacman) [ -f /tmp/pac-updated ] || { pacman-key --init >/dev/null 2>&1; pacman-key --populate >/dev/null 2>&1; pacman -Syu --noconfirm >/dev/null 2>&1; touch /tmp/pac-updated; }
        for p in "$@"; do pacman -S --noconfirm --needed "$p" >/dev/null 2>&1; done ;;
    esac
}
case $pm in
apt-get) install xvfb xauth ca-certificates libgl1-mesa-dri libegl1 libgl1 mesa-vulkan-drivers \
    libx11-6 libxcb1 fonts-dejavu-core fontconfig libasound2t64 libasound2 libpulse0 procps ;;
dnf) install xorg-x11-server-Xvfb mesa-dri-drivers mesa-libEGL mesa-libGL mesa-vulkan-drivers \
    libX11 libglvnd-gles fontconfig dejavu-sans-fonts alsa-lib pulseaudio-libs procps-ng tar gzip findutils ;;
zypper) install xorg-x11-server-Xvfb Mesa-dri Mesa-libEGL1 Mesa-libGL1 libvulkan_lvp libX11-6 \
    dejavu-fonts fontconfig libasound2 libpulse0 procps tar gzip ;;
pacman) install xorg-server-xvfb mesa vulkan-swrast libx11 ttf-dejavu fontconfig alsa-lib libpulse procps-ng ;;
esac
. /etc/os-release
echo "== $PRETTY_NAME, glibc $(getconf GNU_LIBC_VERSION | cut -d' ' -f2)"
[ -s /etc/machine-id ] || tr -d - </proc/sys/kernel/random/uuid >/etc/machine-id
export XDG_RUNTIME_DIR=/tmp/xdg HOME=/tmp/home APPIMAGE_EXTRACT_AND_RUN=1
mkdir -p "$XDG_RUNTIME_DIR" "$HOME" && chmod 700 "$XDG_RUNTIME_DIR"
Xvfb :77 -screen 0 1280x720x24 >/tmp/xvfb.log 2>&1 &
sleep 2; [ -e /tmp/.X11-unix/X77 ] || { echo "Xvfb did not start:"; tail -n 3 /tmp/xvfb.log; }
check() { # $1 name, $2 launcher
    name=$1 app=$2
    echo "== $name"
    out="$(cd /tmp && timeout 120 "$app" --version 2>&1)"; code=$?
    echo "$out" | tail -n 3 | sed 's/^/   /'
    if echo "$out" | grep -q "needs glibc"; then echo "RESULT: $name glibc-too-old (clear message)"; return; fi
    if [ $code -ne 0 ]; then
        echo "$out" | grep -o -E "GLIBC_[0-9.]+' not found|lib[^ :]*: cannot open shared object file" | sort -u | head -n 4 | sed 's/^/   /'
        if echo "$out" | grep -q "GLIBC_"; then echo "RESULT: $name glibc-too-old (loader error)"
        else echo "RESULT: $name FAIL --version exit $code"; fi
        return
    fi
    out="$(cd /tmp && DISPLAY=:77 GTK_A11Y=none LIBGL_ALWAYS_SOFTWARE=1 EMBYCLIENTPLUS_SMOKE_TEST=1 \
        timeout 90 "$app" --player "$SMOKE_URL" 2>&1)"; code=$?
    echo "$out" | grep -E "^smoke:|cannot open shared|undefined symbol|CRITICAL|GLIBC_" | head -n 6 | sed 's/^/   /'
    if echo "$out" | grep -q "^smoke:" && [ $code -eq 0 ] &&
        ! echo "$out" | grep -q -E "cannot open shared object|CRITICAL|GLIBC_"; then
        echo "RESULT: $name ok (plays)"
    else
        echo "$out" | tail -n 4 | sed 's/^/   | /'
        echo "RESULT: $name FAIL smoke exit $code"
    fi
}
for kind in main compat; do
    [ -f "/in/$kind.AppImage" ] || continue
    cp "/in/$kind.AppImage" /tmp/ && chmod +x "/tmp/$kind.AppImage"
    check "$kind-AppImage" "/tmp/$kind.AppImage"
done
[ -f /in/app.tar.gz ] || exit 0
case $pm in
apt-get) install libgtk-4-1 libadwaita-1-0 libgbm1 libva2 libva-drm2 libva-x11-2 libva-wayland2 \
    libvdpau1 libvulkan1 libpipewire-0.3-0t64 libpipewire-0.3-0 libxss1 libxpresent1 libxv1 \
    libgnutls30t64 libgnutls30 ;;
dnf) install gtk4 libadwaita libglvnd-gles mesa-libgbm libva libvdpau vulkan-loader pipewire-libs libXScrnSaver \
    libXpresent libXv gnutls ;;
zypper) install libgtk-4-1 libadwaita-1-0 libgbm1 libva2 libva-drm2 libva-x11-2 libva-wayland2 \
    libvdpau1 libvulkan1 libpipewire-0_3-0 libXss1 libXpresent1 libXv1 libgnutls30 ;;
pacman) install gtk4 libadwaita libva libvdpau vulkan-icd-loader libpipewire libxss libxpresent libxv gnutls ;;
esac
mkdir -p /tmp/tgz && tar -C /tmp/tgz -xzf /in/app.tar.gz
check tar.gz "$(ls -d /tmp/tgz/*/bin/embyclientplus)"
