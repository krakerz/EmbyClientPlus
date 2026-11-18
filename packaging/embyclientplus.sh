#!/bin/sh
# Starts EmbyClientPlus from the release archive or the main AppImage.
#
# libmpv needs libvsscript.so at startup: the bundled VapourSynth stand-in
# (packaging/vsshim) goes first on the library path, and the app tells it
# whether to load SVP's VapourSynth or the system's (src/svp.rs).
here="$(dirname "$(readlink -f "$0")")"
lib="$here/../lib"

# The archive's libraries need a current glibc; older systems get the compat AppImage.
need="$(cat "$lib/glibc.needs" 2>/dev/null)"
have="$(getconf GNU_LIBC_VERSION 2>/dev/null | cut -d' ' -f2)"
if [ -n "$need" ] && [ -n "$have" ] &&
    [ "$(printf '%s\n%s\n' "$need" "$have" | sort -V | head -n 1)" != "$need" ]; then
    echo "EmbyClientPlus needs glibc $need or newer; this system has $have." >&2
    echo "Use the EmbyClientPlus-*-x86_64-compat.AppImage from the releases page instead." >&2
    exit 1
fi

# lib/fallback holds libraries most systems have too: each is used only when
# the system lacks that soname (so a system copy, maybe newer, wins), and
# libstdc++ also when the system's is older than the bundled libraries need.
# The chosen ones are linked into a folder for this process.
path="$lib/vsshim"
if [ -d "$lib/fallback" ]; then
    run="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}"
    for old in "$run"/embyclientplus-libs.*; do
        [ -d "$old" ] && ! kill -0 "${old##*.}" 2>/dev/null && rm -rf "$old" 2>/dev/null
    done
    use="$run/embyclientplus-libs.$$"
    mkdir -p "$use"
    cache="$( (ldconfig -p || /sbin/ldconfig -p || /usr/sbin/ldconfig -p) 2>/dev/null)"
    cxx="GLIBCXX_$(cat "$lib/fallback/libstdc++.needs" 2>/dev/null)"
    for f in "$lib"/fallback/*.so*; do
        name="${f##*/}"
        sys="$(printf '%s\n' "$cache" | grep -F "	$name (" | grep -m 1 x86-64)"
        sys="${sys##*=> }"
        if [ -n "$sys" ] && { [ "$name" != libstdc++.so.6 ] || grep -qaF "$cxx" "$sys"; }; then
            continue
        fi
        ln -s "$f" "$use/$name"
    done
    path="$path:$use"
fi

LD_LIBRARY_PATH="$path${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export LD_LIBRARY_PATH
# Tells the app it can update itself in place (an AppImage sets $APPIMAGE).
export EMBYCLIENTPLUS_INSTALL=archive

exec "$here/embyclientplus-bin" "$@"
