//! Media keys and the system's "now playing" controls for the music player
//! on Windows (System Media Transport Controls) and macOS (Now Playing),
//! the counterpart of Linux's MPRIS (`mpris`). Set up once music first
//! plays; commands hop over to the main thread, where the player lives.

use std::cell::RefCell;
use std::time::Duration;

use gtk::glib;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

use super::MusicPlayer;

/// Cover art handed over as a file (from the image cache).
const ART_WIDTH: u32 = 560;
/// Seek step for the "seek forward/back" buttons that don't say how far.
const SEEK_STEP: f64 = 10.0;

#[derive(Default)]
struct Shown {
    item_id: Option<String>,
    art: Option<String>,
}

thread_local! {
    static CONTROLS: RefCell<Option<MediaControls>> = const { RefCell::new(None) };
    static SHOWN: RefCell<Shown> = RefCell::default();
}

/// Publishes `music`'s state; sets the controls up the first time music
/// plays.
pub fn update(music: &MusicPlayer) {
    let ready = CONTROLS.with(|controls| controls.borrow().is_some());
    if !ready {
        if !music.is_active() {
            return;
        }
        let Some(controls) = create() else { return };
        CONTROLS.with(|slot| slot.replace(Some(controls)));
    }

    let item = music.current().filter(|_| music.is_active());
    let item_id = item.as_ref().map(|i| i.id.clone());
    let changed_track = SHOWN.with(|shown| {
        let mut shown = shown.borrow_mut();
        let changed = shown.item_id != item_id;
        if changed {
            shown.item_id = item_id.clone();
            shown.art = None;
        }
        changed
    });
    // The cover arrives later; show the rest now.
    if changed_track && let Some(image) = item.as_ref().and_then(|i| i.poster()) {
        let client = music.client().clone();
        let id = item_id.clone();
        glib::spawn_future_local(async move {
            let Some(file) = crate::ui::images::file(client, image, ART_WIDTH).await else {
                return;
            };
            let current = SHOWN.with(|shown| {
                let mut shown = shown.borrow_mut();
                let current = shown.item_id == id;
                if current {
                    shown.art = Some(format!("file://{}", file.display()));
                }
                current
            });
            if current && let Some(music) = super::active() {
                update(&music);
            }
        });
    }

    let art = SHOWN.with(|shown| shown.borrow().art.clone());
    let progress = music.position().map(|p| MediaPosition(seconds(p)));
    let playback = match (item.is_some(), music.is_paused()) {
        (false, _) => MediaPlayback::Stopped,
        (true, true) => MediaPlayback::Paused { progress },
        (true, false) => MediaPlayback::Playing { progress },
    };
    CONTROLS.with(|controls| {
        let mut controls = controls.borrow_mut();
        let Some(controls) = controls.as_mut() else {
            return;
        };
        if let Some(item) = &item {
            let metadata = MediaMetadata {
                title: Some(&item.name),
                album: item.album.as_deref(),
                artist: item.artist(),
                cover_url: art.as_deref(),
                duration: item
                    .run_time_ticks
                    .map(|ticks| seconds(ticks as f64 / crate::playback::TICKS_PER_SECOND as f64)),
            };
            if let Err(e) = controls.set_metadata(metadata) {
                tracing::debug!("media controls: metadata failed: {e:?}");
            }
        }
        if let Err(e) = controls.set_playback(playback) {
            tracing::debug!("media controls: status failed: {e:?}");
        }
    });
}

fn seconds(value: f64) -> Duration {
    Duration::from_secs_f64(value.max(0.0))
}

fn create() -> Option<MediaControls> {
    let config = PlatformConfig {
        display_name: "Emby Client+",
        dbus_name: "embyclientplus",
        hwnd: window_handle(),
    };
    #[cfg(windows)]
    if config.hwnd.is_none() {
        tracing::warn!("media controls: no window to attach them to");
        return None;
    }
    let mut controls = match MediaControls::new(config) {
        Ok(controls) => controls,
        Err(e) => {
            tracing::warn!("media controls unavailable: {e:?}");
            return None;
        }
    };
    if let Err(e) = controls.attach(|event| {
        glib::MainContext::default().invoke(move || {
            if let Some(music) = super::active() {
                handle(&music, event);
            }
        });
    }) {
        tracing::warn!("media controls unavailable: {e:?}");
        return None;
    }
    Some(controls)
}

fn handle(music: &MusicPlayer, event: MediaControlEvent) {
    let paused = music.is_paused();
    match event {
        MediaControlEvent::Toggle => music.toggle_pause(),
        MediaControlEvent::Play if paused => music.toggle_pause(),
        MediaControlEvent::Pause if !paused => music.toggle_pause(),
        MediaControlEvent::Next => music.next(),
        MediaControlEvent::Previous => music.previous(),
        MediaControlEvent::Stop => music.stop(),
        MediaControlEvent::SetPosition(MediaPosition(position)) => {
            music.seek(position.as_secs_f64())
        }
        MediaControlEvent::Seek(direction) => seek_by(music, direction, SEEK_STEP),
        MediaControlEvent::SeekBy(direction, by) => seek_by(music, direction, by.as_secs_f64()),
        _ => {}
    }
}

fn seek_by(music: &MusicPlayer, direction: SeekDirection, by: f64) {
    let Some(position) = music.position() else {
        return;
    };
    let step = match direction {
        SeekDirection::Forward => by,
        SeekDirection::Backward => -by,
    };
    music.seek((position + step).max(0.0));
}

/// Windows: the main window's handle, which the controls belong to.
#[cfg(windows)]
fn window_handle() -> Option<*mut std::ffi::c_void> {
    use gtk::prelude::*;
    let window = gtk::gio::Application::default()?
        .downcast::<gtk::Application>()
        .ok()?
        .active_window()?;
    let surface = window.surface()?;
    let surface = surface.downcast_ref::<gdk4_win32::Win32Surface>()?;
    Some(surface.handle().0)
}

/// macOS doesn't need one.
#[cfg(not(windows))]
fn window_handle() -> Option<*mut std::ffi::c_void> {
    None
}
