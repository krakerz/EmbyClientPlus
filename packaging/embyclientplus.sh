#!/bin/sh
# Starts EmbyClientPlus with SVP's VapourSynth when SVP is installed.
#
# libmpv needs libvsscript.so at startup. SVP's plugins need SVP's own
# VapourSynth (R73) and Python 3.12, so when SVP is there its copies go on
# the library path; otherwise a bundled fallback lets the app start without
# SVP. SVP's mpv/ folder itself is never put on the path: it also holds
# SVP's libmpv.so.2, which would replace ours.
here="$(dirname "$(readlink -f "$0")")"
lib="$here/../lib"

config="${XDG_CONFIG_HOME:-$HOME/.config}/embyclientplus/config.toml"
svp_dir=""
if [ -f "$config" ]; then
    svp_dir="$(sed -n 's/^svp_dir *= *"\(.*\)" *$/\1/p' "$config" | head -n 1)"
fi
[ -n "$svp_dir" ] || svp_dir="${SVP_DIR:-$HOME/SVP4}"

if [ -e "$svp_dir/mpv/libvapoursynth-script.so.0" ]; then
    vs_dir="${XDG_CACHE_HOME:-$HOME/.cache}/embyclientplus/svp-vapoursynth"
    mkdir -p "$vs_dir"
    ln -sfn "$svp_dir/mpv/libvapoursynth-script.so.0" "$vs_dir/libvsscript.so"
    ln -sfn "$svp_dir/mpv/libvapoursynth.so" "$vs_dir/libvapoursynth.so"
    LD_LIBRARY_PATH="$vs_dir:$svp_dir/python${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
else
    LD_LIBRARY_PATH="$lib/vapoursynth-fallback${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
export LD_LIBRARY_PATH
# Tells the app it can update itself in place (an AppImage sets $APPIMAGE).
export EMBYCLIENTPLUS_INSTALL=archive

exec "$here/embyclientplus-bin" "$@"
