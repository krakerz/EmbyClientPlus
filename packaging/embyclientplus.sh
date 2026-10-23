#!/bin/sh
# Starts EmbyClientPlus from the release archive or the main AppImage.
#
# libmpv needs libvsscript.so at startup: the bundled VapourSynth stand-in
# (packaging/vsshim) goes first on the library path, and the app tells it
# whether to load SVP's VapourSynth or the system's (src/svp.rs).
here="$(dirname "$(readlink -f "$0")")"
lib="$here/../lib"

LD_LIBRARY_PATH="$lib/vsshim${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export LD_LIBRARY_PATH
# Tells the app it can update itself in place (an AppImage sets $APPIMAGE).
export EMBYCLIENTPLUS_INSTALL=archive

exec "$here/embyclientplus-bin" "$@"
