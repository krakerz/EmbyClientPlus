fn main() {
    let attrs = tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&["intercept_playback", "set_server_url"]),
    );
    tauri_build::try_build(attrs).expect("tauri build failed");
}
