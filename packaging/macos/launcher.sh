#!/bin/sh
# Contents/MacOS/embyclientplus in the app bundle: points GTK at the data
# bundled under Contents/Resources, then starts the real binary.
contents="$(cd "$(dirname "$0")/.." && pwd)"
res="$contents/Resources"

export XDG_DATA_DIRS="$res/share${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
export GSETTINGS_SCHEMA_DIR="$res/share/glib-2.0/schemas"

# gdk-pixbuf's loader cache holds absolute paths, so it is written for
# wherever the bundle is now (once per location and version).
loaders="$res/lib/gdk-pixbuf-2.0/2.10.0"
cache_dir="$HOME/Library/Caches/EmbyClientPlus"
cache="$cache_dir/loaders-$(echo "$contents" | cksum | cut -d' ' -f1).cache"
if [ ! -f "$cache" ] || [ "$loaders/loaders.cache.in" -nt "$cache" ]; then
    mkdir -p "$cache_dir"
    sed "s|@LOADERS@|$loaders/loaders|g" "$loaders/loaders.cache.in" > "$cache"
fi
export GDK_PIXBUF_MODULE_FILE="$cache"

exec "$contents/MacOS/embyclientplus-bin" "$@"
