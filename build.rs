fn main() {
    // subtitles.rs hand-rolls its own libass FFI (no working binding
    // crate exists — see its module doc comment); assumed present on
    // the system, same as mpv's own dependency on it.
    println!("cargo:rustc-link-lib=ass");

    let attrs = tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["intercept_playback", "set_server_url"]),
    );
    tauri_build::try_build(attrs).expect("tauri build failed");
}
