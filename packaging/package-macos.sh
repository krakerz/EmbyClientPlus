#!/usr/bin/env bash
# Builds the macOS app bundle (unsigned, arm64) into dist/ from an already
# built target/release/embyclientplus. GTK, libadwaita and libmpv come from
# Homebrew (CI: macos-14).
#
#   Emby Client+.app/Contents/
#     MacOS/      launcher (packaging/macos/launcher.sh) + embyclientplus-bin
#     Frameworks/ every Homebrew dylib it needs, the VapourSynth stand-ins
#     Resources/  icon, GSettings schemas, Adwaita icons, gdk-pixbuf loaders
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
BREW="$(brew --prefix)"
BIN="target/release/embyclientplus"
DIST="dist"
app="$DIST/Emby Client+.app"
contents="$app/Contents"

[ -x "$BIN" ] || { echo "missing $BIN (cargo build --release)" >&2; exit 1; }
rm -rf "$app"
mkdir -p "$contents/MacOS" "$contents/Frameworks" "$contents/Resources"
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist > "$contents/Info.plist"
install -m755 packaging/macos/launcher.sh "$contents/MacOS/embyclientplus"
install -m755 "$BIN" "$contents/MacOS/embyclientplus-bin"

# The app icon, from the SVG.
iconset="$DIST/EmbyClientPlus.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    rsvg-convert -w "$size" -h "$size" packaging/io.github.krakerz.EmbyClientPlus.svg \
        -o "$iconset/icon_${size}x${size}.png"
    rsvg-convert -w "$((size * 2))" -h "$((size * 2))" packaging/io.github.krakerz.EmbyClientPlus.svg \
        -o "$iconset/icon_${size}x${size}@2x.png"
done
iconutil -c icns "$iconset" -o "$contents/Resources/EmbyClientPlus.icns"
rm -rf "$iconset"

# VapourSynth must come from the user's install (SVP on macOS uses
# Homebrew's) at run time, through the stand-in (packaging/vsshim).
is_vapoursynth() {
    case "$(basename "$1")" in
        *vapoursynth*|*vsscript*|*python*|Python) return 0 ;;
    esac
    return 1
}

# Copies a binary's Homebrew dylibs into Frameworks, recursively, and
# points its load commands at them.
declare -A copied=()
bundle_deps() {
    local file="$1" dep name
    while read -r dep; do
        case "$dep" in "$BREW"/*|@rpath/*) ;; *) continue ;; esac
        name="$(basename "$dep")"
        if is_vapoursynth "$dep"; then
            if [ -z "${copied[$name]:-}" ]; then
                copied[$name]=1
                echo "VapourSynth stand-in: $name"
                clang -shared -O2 -arch arm64 -install_name "@rpath/$name" \
                    -o "$contents/Frameworks/$name" packaging/vsshim/vsshim.c
            fi
        elif [ -z "${copied[$name]:-}" ]; then
            copied[$name]=1
            local src="$dep"
            [ -e "$src" ] || src="$(find "$BREW/lib" "$BREW/opt" -name "$name" -print -quit 2>/dev/null)"
            install -m755 "$(realpath "$src")" "$contents/Frameworks/$name"
            install_name_tool -id "@rpath/$name" "$contents/Frameworks/$name"
            bundle_deps "$contents/Frameworks/$name"
        fi
        install_name_tool -change "$dep" "@rpath/$name" "$file"
    done < <(otool -L "$file" | tail -n +2 | awk '{print $1}' | grep -v "^$(otool -D "$file" | tail -n +2)$" || true)
}
bundle_deps "$contents/MacOS/embyclientplus-bin"
install_name_tool -add_rpath "@executable_path/../Frameworks" "$contents/MacOS/embyclientplus-bin"
for lib in "$contents"/Frameworks/*.dylib; do
    install_name_tool -add_rpath "@loader_path" "$lib" 2>/dev/null || true
done

# GTK runtime data.
share="$contents/Resources/share"
mkdir -p "$share/glib-2.0/schemas" "$share/icons/Adwaita" "$share/icons/hicolor"
cp "$BREW"/share/glib-2.0/schemas/org.gtk.gtk4.*.gschema.xml "$share/glib-2.0/schemas/"
glib-compile-schemas "$share/glib-2.0/schemas"
cp "$BREW/share/icons/Adwaita/index.theme" "$share/icons/Adwaita/"
cp -r "$BREW/share/icons/Adwaita/symbolic" "$share/icons/Adwaita/"
cp "$BREW/share/icons/hicolor/index.theme" "$share/icons/hicolor/"

# gdk-pixbuf loaders, with a cache template the launcher fills in.
loaders="$contents/Resources/lib/gdk-pixbuf-2.0/2.10.0"
mkdir -p "$loaders/loaders"
cp "$BREW"/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.so "$loaders/loaders/"
for loader in "$loaders"/loaders/*.so; do
    bundle_deps "$loader"
    install_name_tool -add_rpath "@loader_path/../../../../../Frameworks" "$loader" 2>/dev/null || true
done
GDK_PIXBUF_MODULEDIR="$loaders/loaders" gdk-pixbuf-query-loaders |
    sed "s|$(cd "$loaders/loaders" && pwd)|@LOADERS@|g" > "$loaders/loaders.cache.in"

# Lets the app update itself in place (src/update.rs).
touch "$contents/.embyclientplus-install"

# Changed binaries lose their signature; arm64 macOS won't run unsigned
# code, so sign ad hoc (not notarized: first launch needs right-click → Open).
codesign --force --deep --sign - "$app"

tar -C "$DIST" -czf "$DIST/embyclientplus-$VERSION-macos-arm64.tar.gz" "Emby Client+.app"
ls -l "$DIST"
