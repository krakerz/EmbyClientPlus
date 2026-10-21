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

# svp_detect (shared with the compat AppImage's hook) finds SVP's folder.
. "$lib/svp-detect.sh"
if svp_detect; then
    LD_LIBRARY_PATH="$SVP_LIB_PATH${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
else
    LD_LIBRARY_PATH="$lib/vapoursynth-fallback${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
fi
export LD_LIBRARY_PATH
# Tells the app it can update itself in place (an AppImage sets $APPIMAGE).
export EMBYCLIENTPLUS_INSTALL=archive

exec "$here/embyclientplus-bin" "$@"
