//! Steam's on-screen keyboard for text fields driven by the controller;
//! Game Mode only (`available()` is false elsewhere). Follows BrowDeck's
//! `osk.rs` (same author). Steam handles `steam://open/keyboard` /
//! `steam://close/keyboard` URLs, which is how SDL opens it for games.

use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// Steam's Game Mode session sets `SDL_ENABLE_STEAM_SCREEN_KEYBOARD=1` for
/// what it launches; the desktop client doesn't, so the keyboard isn't
/// forced on a desktop user. `SteamDeck=1` is SDL's own check, kept for
/// stock SteamOS. The environment doesn't change, so it's read once.
pub fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let set = |name| std::env::var(name).is_ok_and(|v| v == "1");
        set("SDL_ENABLE_STEAM_SCREEN_KEYBOARD") || set("SteamDeck")
    })
}

/// `field` is the text field's rect in physical pixels, relative to the
/// window — Steam keeps the keyboard from covering it where it can.
pub fn open(field: Option<[i32; 4]>) {
    send(&open_url(field));
}

pub fn close() {
    send("steam://close/keyboard");
}

/// `Mode=0` is Steam's single-line mode: Enter submits and dismisses.
fn open_url(field: Option<[i32; 4]>) -> String {
    match field {
        Some([x, y, w, h]) => {
            format!("steam://open/keyboard?XPosition={x}&YPosition={y}&Width={w}&Height={h}&Mode=0")
        }
        None => "steam://open/keyboard?Mode=0".to_string(),
    }
}

/// Through the `steam` script, which hands the URL to the running client —
/// confirmed to open the keyboard in Game Mode, where routing through
/// `xdg-open` depends on the session's URL-handler setup.
fn send(url: &str) {
    let steam = std::env::var("STEAMSCRIPT").unwrap_or_else(|_| "steam".to_string());
    // Steam's overlay/runtime libraries would otherwise leak into the
    // script.
    let spawned = Command::new(steam)
        .arg(url)
        .env_remove("LD_PRELOAD")
        .env_remove("LD_LIBRARY_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => tracing::warn!("couldn't send {url} to Steam: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_url_carries_the_field_rect_like_sdl() {
        assert_eq!(
            open_url(Some([10, 700, 300, 40])),
            "steam://open/keyboard?XPosition=10&YPosition=700&Width=300&Height=40&Mode=0"
        );
        assert_eq!(open_url(None), "steam://open/keyboard?Mode=0");
    }
}
