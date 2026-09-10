use tauri::{Url, WebviewUrl, WebviewWindowBuilder};

use crate::config::Settings;
use crate::db::Db;
use crate::emby::EmbyClient;

const INJECT_SCRIPT: &str = include_str!("../ui/inject.js");
const DEVICE_ID: &str = "embyclientplus-linux";

/// Fired by `ui/inject.js` once it detects both a video-player route and
/// a readable access token. Never logs `access_token` (never print an
/// access token).
#[tauri::command]
pub async fn intercept_playback(
    item_id: String,
    access_token: String,
    user_id: Option<String>,
    window: tauri::WebviewWindow,
) -> Result<(), String> {
    tracing::info!(item_id = %item_id, has_user_id = user_id.is_some(), "playback route intercepted");

    let user_id = user_id.ok_or_else(|| "no user id available from the webview".to_string())?;
    let settings = Settings::load().map_err(|e| e.to_string())?;
    if settings.server.url.is_empty() {
        return Err("no server configured".to_string());
    }
    let emby = EmbyClient::new(settings.server.url.clone(), DEVICE_ID).with_token(access_token);

    let override_key = crate::playback::resolve_override_key(&emby, &user_id, &item_id)
        .await
        .map_err(|e| e.to_string())?;
    // `resolve_override_key` returns the item's own id for movies (no
    // SeriesId found) and the series id otherwise — mirror that here
    // rather than re-deriving it from scratch.
    let item_type = if override_key == item_id {
        crate::db::ItemType::Movie
    } else {
        crate::db::ItemType::Series
    };
    let title_override = {
        let db = Db::open_default().map_err(|e| e.to_string())?;
        db.get_override(&override_key).map_err(|e| e.to_string())?
    };

    crate::playback::start_playback(
        emby,
        title_override,
        &settings,
        &user_id,
        &item_id,
        window,
        override_key,
        item_type,
    )
    .await
    .map_err(|e| e.to_string())
}

/// Called from `ui/connect.html` on first run, before any server is
/// configured. Saves the URL and redirects the same window to Emby's own
/// web client at that address, rather than opening a new window.
#[tauri::command]
pub fn set_server_url(url: String, window: tauri::WebviewWindow) -> Result<(), String> {
    let mut settings = Settings::load().map_err(|e| e.to_string())?;
    settings.server.url = url.trim_end_matches('/').to_string();
    settings.save().map_err(|e| e.to_string())?;

    let web_url: Url = format!("{}/web/index.html", settings.server.url)
        .parse()
        .map_err(|e| format!("invalid server url: {e}"))?;
    window.navigate(web_url).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn create_main_window(app: &tauri::App) -> tauri::Result<()> {
    let settings = Settings::load().unwrap_or_default();
    let initial_url = if settings.server.url.is_empty() {
        WebviewUrl::App("connect.html".into())
    } else {
        let web_url = format!("{}/web/index.html", settings.server.url);
        WebviewUrl::External(
            web_url
                .parse()
                .expect("server url stored in config.toml should already be valid"),
        )
    };

    WebviewWindowBuilder::new(app, "main", initial_url)
        .title("EmbyClientPlus")
        .inner_size(1280.0, 800.0)
        .initialization_script(INJECT_SCRIPT)
        .build()?;
    Ok(())
}
