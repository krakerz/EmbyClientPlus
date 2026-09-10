// Keyring-backed token persistence — not yet wired into the login flow
// (which currently always reads a fresh token from the webview via
// ui/inject.js); would let a future version skip needing the user
// already logged into the embedded webview on every launch.
#[allow(dead_code)]
mod auth;
mod config;
mod db;
mod emby;
mod frame_gen;
mod playback;
mod webview_bridge;

fn main() {
    tracing_subscriber::fmt::init();

    // M1's mpv embedding reparents an X11 child window into the main
    // window, which only works if both are in the same protocol domain.
    // This app's main window renders as a native Wayland surface by
    // default under this session type, which cannot host that at all —
    // confirmed live in M7 (see NOTES.md). Must be set before Tauri/GTK
    // initializes.
    if std::env::var_os("GDK_BACKEND").is_none() {
        // SAFETY: single-threaded at this point, before Tauri/GTK init.
        unsafe {
            std::env::set_var("GDK_BACKEND", "x11");
        }
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            webview_bridge::intercept_playback,
            webview_bridge::set_server_url,
        ])
        .setup(|app| {
            webview_bridge::create_main_window(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
