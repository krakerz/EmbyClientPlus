#!/usr/bin/env bash
# Builds the Windows release zip into dist/ from an already built
# target/release/embyclientplus.exe. Runs in an MSYS2 UCRT64 shell (CI),
# where GTK, libadwaita and libmpv come from MSYS2's packages.
#
# Layout (GTK finds its data relative to its DLLs' folder):
#   EmbyClientPlus-<ver>-windows-x86_64/
#     bin/    embyclientplus.exe, every DLL it needs, the VapourSynth stand-ins
#     lib/    gdk-pixbuf loaders
#     share/  GSettings schemas, Adwaita symbolic icons
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
PREFIX="${MINGW_PREFIX:-/ucrt64}"
EXE="target/release/embyclientplus.exe"
DIST="dist"
bundle="EmbyClientPlus-$VERSION-windows-x86_64"
root="$DIST/$bundle"

[ -f "$EXE" ] || { echo "missing $EXE (cargo build --release)" >&2; exit 1; }
rm -rf "$root"
mkdir -p "$root/bin" "$root/lib" "$root/share"
cp "$EXE" "$root/bin/"

# The DLLs a binary needs from the MSYS2 prefix, recursively.
deps() {
    ntldd -R "$@" 2>/dev/null |
        sed -n 's/.*=> \(.*\.dll\) (0x.*/\1/p' |
        tr '\\' '/' |
        while read -r dll; do
            dll="$(cygpath -u "$dll")"
            case "$dll" in "$PREFIX"/bin/*) echo "$dll" ;; esac
        done | sort -u
}

# VapourSynth must come from SVP at run time: its DLLs are replaced by the
# stand-in (packaging/vsshim), built once per name libmpv imports.
is_vapoursynth() {
    case "$(basename "$1" | tr '[:upper:]' '[:lower:]')" in
        *vsscript*|*vapoursynth*) return 0 ;;
    esac
    return 1
}
libmpv="$(ls "$PREFIX"/bin/libmpv-*.dll | head -n 1)"
vs_names=()
for dll in $(deps "$EXE" "$libmpv") "$libmpv"; do
    if is_vapoursynth "$dll"; then
        vs_names+=("$(basename "$dll")")
    else
        cp -n "$dll" "$root/bin/"
    fi
done
# Python comes with SVP's VapourSynth too; drop it if it slipped in.
rm -f "$root"/bin/libpython*.dll
[ "${#vs_names[@]}" -gt 0 ] || { echo "libmpv links no VapourSynth DLL — no SVP support?" >&2; exit 1; }
for name in "${vs_names[@]}"; do
    echo "VapourSynth stand-in: $name"
    gcc -shared -O2 -o "$root/bin/$name" packaging/vsshim/vsshim.c
done

# gdk-pixbuf loaders (and their DLLs), with a cache pointing at them.
pixbuf_dir="$(ls -d "$PREFIX"/lib/gdk-pixbuf-2.0/2.10.0)"
mkdir -p "$root/lib/gdk-pixbuf-2.0/2.10.0"
cp -r "$pixbuf_dir/loaders" "$root/lib/gdk-pixbuf-2.0/2.10.0/"
for dll in $(deps "$root"/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.dll); do
    cp -n "$dll" "$root/bin/"
done
(cd "$root" && GDK_PIXBUF_MODULEDIR=lib/gdk-pixbuf-2.0/2.10.0/loaders \
    gdk-pixbuf-query-loaders > lib/gdk-pixbuf-2.0/2.10.0/loaders.cache)

# GTK runtime data: compiled GSettings schemas and the symbolic icons.
schemas="$root/share/glib-2.0/schemas"
mkdir -p "$schemas"
cp "$PREFIX"/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$schemas/"
glib-compile-schemas "$schemas"
icons="$root/share/icons"
mkdir -p "$icons/Adwaita" "$icons/hicolor"
cp "$PREFIX/share/icons/Adwaita/index.theme" "$icons/Adwaita/"
cp -r "$PREFIX/share/icons/Adwaita/symbolic" "$icons/Adwaita/"
cp "$PREFIX/share/icons/hicolor/index.theme" "$icons/hicolor/"

# Lets the app update itself in place (src/update.rs).
touch "$root/.embyclientplus-install"
cp LICENSE "$root/" 2>/dev/null || true

(cd "$DIST" && zip -qr "embyclientplus-$VERSION-windows-x86_64.zip" "$bundle")
rm -rf "$root"
ls -l "$DIST"
