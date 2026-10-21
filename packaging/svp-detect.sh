# Shared by packaging/embyclientplus.sh (archive, main AppImage) and
# packaging/appimage/svp.src.hook (compat AppImage). Sourced, POSIX sh.
#
# svp_detect: succeeds when SVP is installed and sets
#   SVP_LIB_PATH  SVP's VapourSynth (as libvsscript.so) and its Python 3.12
#                 folder, for the dynamic loader's search path.
#
# libmpv needs libvsscript.so at startup. SVP's plugins need SVP's own
# VapourSynth (R73) and Python 3.12, so when SVP is there its copies go on
# the library path; otherwise the caller falls back to a bundled stand-in.
# SVP's mpv/ folder itself is never put on the path: it also holds SVP's
# libmpv.so.2, which would replace ours.
svp_detect() {
    SVP_LIB_PATH=""
    config="${XDG_CONFIG_HOME:-$HOME/.config}/embyclientplus/config.toml"
    svp_dir=""
    if [ -f "$config" ]; then
        svp_dir="$(sed -n 's/^svp_dir *= *"\(.*\)" *$/\1/p' "$config" | head -n 1)"
    fi
    [ -n "$svp_dir" ] || svp_dir="${SVP_DIR:-$HOME/SVP4}"

    [ -e "$svp_dir/mpv/libvapoursynth-script.so.0" ] || return 1
    vs_dir="${XDG_CACHE_HOME:-$HOME/.cache}/embyclientplus/svp-vapoursynth"
    mkdir -p "$vs_dir"
    ln -sfn "$svp_dir/mpv/libvapoursynth-script.so.0" "$vs_dir/libvsscript.so"
    ln -sfn "$svp_dir/mpv/libvapoursynth.so" "$vs_dir/libvapoursynth.so"
    SVP_LIB_PATH="$vs_dir:$svp_dir/python"
}
