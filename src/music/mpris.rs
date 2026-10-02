//! MPRIS for the music player: desktop media controls, media keys and
//! Steam's overlay see what's playing and can drive it. Served on the
//! session bus from the tokio runtime once music first plays; commands
//! hop over to the GTK main thread, where the player lives.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use gtk::glib;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};
use zbus::{fdo, interface};

use super::{MusicPlayer, Repeat};
use crate::player::Player;
use crate::runtime::spawn_detached;

const PATH: &str = "/org/mpris/MediaPlayer2";
const BUS_NAME: &str = "org.mpris.MediaPlayer2.EmbyClientPlus";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
const MICROS: f64 = 1_000_000.0;
/// Cover art handed out as a file (from the image cache).
const ART_WIDTH: u32 = 560;

/// What MPRIS reports, mirrored from the main thread.
#[derive(Debug, Clone, PartialEq)]
struct State {
    status: &'static str,
    item_id: Option<String>,
    title: String,
    artist: String,
    album: String,
    length_us: i64,
    art: Option<String>,
    shuffle: bool,
    loop_status: &'static str,
    can_go_next: bool,
    can_go_previous: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            status: "Stopped",
            item_id: None,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            length_us: 0,
            art: None,
            shuffle: false,
            loop_status: "None",
            can_go_next: false,
            can_go_previous: false,
        }
    }
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static CONNECTION: OnceLock<zbus::Connection> = OnceLock::new();
static STARTED: OnceLock<()> = OnceLock::new();
/// For `Position` and `Volume`, read on the bus thread (mpv is thread-safe).
static PLAYER: OnceLock<Player> = OnceLock::new();

fn state() -> State {
    STATE
        .lock()
        .ok()
        .and_then(|state| state.clone())
        .unwrap_or_default()
}

/// Publishes `music`'s state; starts the MPRIS server the first time music
/// plays.
pub fn update(music: &MusicPlayer) {
    if !music.is_active() && STARTED.get().is_none() {
        return;
    }
    let _ = PLAYER.set(music.inner.player);
    if STARTED.set(()).is_ok() {
        spawn_detached(async {
            if let Err(e) = serve().await {
                tracing::warn!("MPRIS unavailable: {e:#}");
            }
        });
    }

    let previous = state();
    let mut next = snapshot(music);
    // Keep the cover while the track stays the same; fetch it when it changes.
    if next.item_id == previous.item_id {
        next.art = previous.art.clone();
    } else if let Some(image) = music.current().and_then(|item| item.poster()) {
        let client = music.client().clone();
        let id = next.item_id.clone();
        glib::spawn_future_local(async move {
            let Some(file) = crate::ui::images::file(client, image, ART_WIDTH).await else {
                return;
            };
            let changed = {
                let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
                match state.as_mut() {
                    Some(state) if state.item_id == id => {
                        state.art = Some(format!("file://{}", file.display()));
                        true
                    }
                    _ => false,
                }
            };
            if changed {
                spawn_detached(emit());
            }
        });
    }
    if next != previous {
        *STATE.lock().unwrap_or_else(|e| e.into_inner()) = Some(next);
        spawn_detached(emit());
    }
}

fn snapshot(music: &MusicPlayer) -> State {
    if !music.is_active() {
        return State::default();
    }
    let item = music.current();
    let queue = music.queue();
    let repeat = music.repeat();
    State {
        status: match (music.inner.current.borrow().is_some(), music.is_paused()) {
            (false, _) => "Stopped",
            (true, true) => "Paused",
            (true, false) => "Playing",
        },
        item_id: item.as_ref().map(|i| i.id.clone()),
        title: item.as_ref().map(|i| i.name.clone()).unwrap_or_default(),
        artist: item
            .as_ref()
            .and_then(|i| i.artist())
            .unwrap_or_default()
            .to_string(),
        album: item
            .as_ref()
            .and_then(|i| i.album.clone())
            .unwrap_or_default(),
        length_us: item
            .as_ref()
            .and_then(|i| i.run_time_ticks)
            .map_or(0, |ticks| ticks / 10),
        art: None,
        shuffle: queue.shuffled(),
        loop_status: match repeat {
            Repeat::Off => "None",
            Repeat::All => "Playlist",
            Repeat::One => "Track",
        },
        can_go_next: queue.upcoming(repeat).is_some(),
        can_go_previous: !queue.is_empty(),
    }
}

async fn serve() -> zbus::Result<()> {
    let builder = || {
        zbus::connection::Builder::session()?
            .serve_at(PATH, Root)?
            .serve_at(PATH, PlayerInterface)
    };
    // A second running copy gets its own name, as MPRIS asks.
    let connection = match builder()?.name(BUS_NAME)?.build().await {
        Ok(connection) => connection,
        Err(_) => {
            let name = format!("{BUS_NAME}.instance{}", std::process::id());
            builder()?.name(name)?.build().await?
        }
    };
    let _ = CONNECTION.set(connection);
    emit().await;
    Ok(())
}

/// Sends PropertiesChanged for everything that may have changed.
async fn emit() {
    let Some(connection) = CONNECTION.get() else {
        return;
    };
    let Ok(iface) = connection
        .object_server()
        .interface::<_, PlayerInterface>(PATH)
        .await
    else {
        return;
    };
    let emitter = iface.signal_emitter();
    let player = iface.get().await;
    let results = [
        player.playback_status_changed(emitter).await,
        player.metadata_changed(emitter).await,
        player.shuffle_changed(emitter).await,
        player.loop_status_changed(emitter).await,
        player.can_go_next_changed(emitter).await,
        player.can_go_previous_changed(emitter).await,
    ];
    if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
        tracing::debug!("MPRIS update failed: {e}");
    }
}

/// Runs `act` on the main thread with the active music player.
fn on_main(act: impl FnOnce(&MusicPlayer) + Send + 'static) {
    glib::MainContext::default().invoke(move || {
        if let Some(music) = super::active() {
            act(&music);
        }
    });
}

struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}

    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> &str {
        "EmbyClientPlus"
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> &str {
        "io.github.krakerz.EmbyClientPlus"
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct PlayerInterface;

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl PlayerInterface {
    fn next(&self) {
        on_main(MusicPlayer::next);
    }

    fn previous(&self) {
        on_main(MusicPlayer::previous);
    }

    fn pause(&self) {
        on_main(|music| {
            if !music.is_paused() {
                music.toggle_pause();
            }
        });
    }

    fn play_pause(&self) {
        on_main(MusicPlayer::toggle_pause);
    }

    fn stop(&self) {
        on_main(MusicPlayer::stop);
    }

    fn play(&self) {
        on_main(|music| {
            if music.is_paused() {
                music.toggle_pause();
            }
        });
    }

    async fn seek(
        &self,
        offset: i64,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let position = PLAYER.get().and_then(|p| p.position()).unwrap_or(0.0);
        let target = (position + offset as f64 / MICROS).max(0.0);
        on_main(move |music| music.seek(target));
        let _ = Self::seeked(&emitter, (target * MICROS) as i64).await;
        Ok(())
    }

    async fn set_position(
        &self,
        track_id: ObjectPath<'_>,
        position: i64,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        // Ignored unless it's about the current track, as the spec says.
        if track_id.as_str() != track_path(&state()).as_str() || position < 0 {
            return Ok(());
        }
        let target = position as f64 / MICROS;
        on_main(move |music| music.seek(target));
        let _ = Self::seeked(&emitter, position).await;
        Ok(())
    }

    fn open_uri(&self, _uri: &str) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(
            "opening URIs isn't supported".into(),
        ))
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> &'static str {
        state().status
    }

    #[zbus(property)]
    fn loop_status(&self) -> &'static str {
        state().loop_status
    }

    #[zbus(property)]
    fn set_loop_status(&mut self, status: String) {
        let wanted = match status.as_str() {
            "Playlist" => Repeat::All,
            "Track" => Repeat::One,
            _ => Repeat::Off,
        };
        on_main(move |music| {
            // Repeat only cycles; step until it lands on the wanted mode.
            for _ in 0..3 {
                if music.repeat() == wanted {
                    break;
                }
                music.cycle_repeat();
            }
        });
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        state().shuffle
    }

    #[zbus(property)]
    fn set_shuffle(&mut self, on: bool) {
        on_main(move |music| music.set_shuffle(on));
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn set_rate(&mut self, _rate: f64) {}

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata(&state())
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        PLAYER.get().map_or(1.0, |p| p.volume() / 100.0)
    }

    #[zbus(property)]
    fn set_volume(&mut self, volume: f64) {
        let volume = (volume * 100.0).clamp(0.0, 100.0);
        on_main(move |music| music.set_volume(volume));
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        PLAYER
            .get()
            .and_then(|p| p.position())
            .map_or(0, |seconds| (seconds * MICROS) as i64)
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        state().can_go_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        state().can_go_previous
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        state().item_id.is_some()
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        state().item_id.is_some()
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        state().item_id.is_some()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}

/// The track's D-Bus object path (Emby ids are hex, so they're valid).
fn track_path(state: &State) -> String {
    match &state.item_id {
        Some(id) if id.chars().all(|c| c.is_ascii_alphanumeric()) => {
            format!("/io/github/krakerz/EmbyClientPlus/track/{id}")
        }
        _ => NO_TRACK.to_string(),
    }
}

fn metadata(state: &State) -> HashMap<String, OwnedValue> {
    let mut map = HashMap::new();
    let mut put = |key: &str, value: Value<'_>| {
        if let Ok(value) = OwnedValue::try_from(value) {
            map.insert(key.to_string(), value);
        }
    };
    if let Ok(path) = ObjectPath::try_from(track_path(state)) {
        put("mpris:trackid", Value::from(path));
    }
    if state.item_id.is_none() {
        return map;
    }
    put("xesam:title", Value::from(state.title.clone()));
    if !state.artist.is_empty() {
        put("xesam:artist", Value::from(vec![state.artist.clone()]));
    }
    if !state.album.is_empty() {
        put("xesam:album", Value::from(state.album.clone()));
    }
    if state.length_us > 0 {
        put("mpris:length", Value::from(state.length_us));
    }
    if let Some(art) = &state.art {
        put("mpris:artUrl", Value::from(art.clone()));
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_without_a_track_is_just_the_no_track_id() {
        let map = metadata(&State::default());
        assert_eq!(map.len(), 1);
        assert!(map.contains_key("mpris:trackid"));
    }

    #[test]
    fn metadata_for_a_track() {
        let state = State {
            status: "Playing",
            item_id: Some("abc123".into()),
            title: "Song".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            length_us: 180_000_000,
            art: Some("file:///tmp/x".into()),
            ..State::default()
        };
        let map = metadata(&state);
        for key in [
            "mpris:trackid",
            "xesam:title",
            "xesam:artist",
            "xesam:album",
            "mpris:length",
            "mpris:artUrl",
        ] {
            assert!(map.contains_key(key), "{key}");
        }
        assert_eq!(
            track_path(&state),
            "/io/github/krakerz/EmbyClientPlus/track/abc123"
        );
    }

    #[test]
    fn odd_ids_fall_back_to_no_track() {
        let state = State {
            item_id: Some("a-b".into()),
            ..State::default()
        };
        assert_eq!(track_path(&state), NO_TRACK);
    }
}
