//! The music player engine: a play queue mirrored into mpv as "current +
//! next" so tracks run gaplessly, with Emby session reporting. Shares the
//! one mpv with video (starting either stops the other) and never touches
//! SVP.

pub mod mpris;
pub mod queue;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use gtk::glib;

use crate::emby::EmbyClient;
use crate::emby::models::{BaseItem, ProgressRequest, StoppedRequest};
use crate::emby::playback_info::StreamRequest;
use crate::player::{END_FILE_REASON_EOF, Player, PlayerEvent};
use crate::runtime::{spawn_detached, spawn_tokio};
pub use queue::{Queue, Repeat};

const TICKS_PER_SECOND: f64 = 10_000_000.0;
const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
/// "Previous" restarts the track instead when this far in.
const RESTART_THRESHOLD: f64 = 3.0;

thread_local! {
    /// The active engine, for player events (main thread only).
    static ACTIVE: RefCell<Weak<Inner>> = const { RefCell::new(Weak::new()) };
}

/// Hands a player event to the music engine; true when music is playing
/// (the event is then the engine's alone).
pub fn dispatch(event: &PlayerEvent) -> bool {
    match ACTIVE.with(|active| active.borrow().upgrade()) {
        Some(inner) if inner.active.get() => {
            MusicPlayer { inner }.handle(event);
            true
        }
        _ => false,
    }
}

/// The live music player (main thread only).
fn active() -> Option<MusicPlayer> {
    ACTIVE
        .with(|active| active.borrow().upgrade())
        .map(|inner| MusicPlayer { inner })
}

/// A track resolved for playback.
#[derive(Clone)]
struct Track {
    item: BaseItem,
    url: String,
    media_source_id: String,
    play_session_id: String,
}

#[derive(Clone)]
pub struct MusicPlayer {
    inner: Rc<Inner>,
}

#[derive(Clone)]
pub struct WeakMusicPlayer(Weak<Inner>);

impl WeakMusicPlayer {
    pub fn upgrade(&self) -> Option<MusicPlayer> {
        self.0.upgrade().map(|inner| MusicPlayer { inner })
    }
}

struct Inner {
    client: Arc<EmbyClient>,
    user_id: String,
    player: Player,
    queue: RefCell<Queue>,
    repeat: Cell<Repeat>,
    current: RefCell<Option<Track>>,
    /// Whether the upcoming track is queued in mpv after the current one.
    next_queued: Cell<bool>,
    /// The upcoming track being resolved; a result for any other is stale.
    next_wanted: RefCell<Option<String>>,
    active: Cell<bool>,
    paused: Cell<bool>,
    /// Bumped on every load, so late async results for an older one drop.
    generation: Cell<u64>,
    timer: RefCell<Option<glib::SourceId>>,
    listeners: RefCell<Vec<Box<dyn Fn()>>>,
}

impl MusicPlayer {
    pub fn new(client: Arc<EmbyClient>, user_id: String, player: Player) -> Self {
        let inner = Rc::new(Inner {
            client,
            user_id,
            player,
            queue: RefCell::new(Queue::default()),
            repeat: Cell::new(Repeat::Off),
            current: RefCell::new(None),
            next_queued: Cell::new(false),
            next_wanted: RefCell::new(None),
            active: Cell::new(false),
            paused: Cell::new(false),
            generation: Cell::new(0),
            timer: RefCell::new(None),
            listeners: RefCell::new(Vec::new()),
        });
        ACTIVE.with(|active| *active.borrow_mut() = Rc::downgrade(&inner));
        MusicPlayer { inner }
    }

    /// Runs `listener` whenever what's playing (or the queue) changes.
    pub fn on_change(&self, listener: impl Fn() + 'static) {
        self.inner.listeners.borrow_mut().push(Box::new(listener));
    }

    fn changed(&self) {
        // Listeners may call back into the player; don't hold the borrow.
        let listeners = std::mem::take(&mut *self.inner.listeners.borrow_mut());
        for listener in &listeners {
            listener();
        }
        let mut current = self.inner.listeners.borrow_mut();
        let added = std::mem::take(&mut *current);
        *current = listeners;
        current.extend(added);
        drop(current);
        mpris::update(self);
    }

    pub fn downgrade(&self) -> WeakMusicPlayer {
        WeakMusicPlayer(Rc::downgrade(&self.inner))
    }

    pub fn volume(&self) -> f64 {
        self.inner.player.volume()
    }

    pub fn set_volume(&self, volume: f64) {
        if let Err(e) = self.inner.player.set_volume(volume) {
            tracing::warn!("{e:#}");
        }
    }

    pub fn is_active(&self) -> bool {
        self.inner.active.get()
    }

    pub fn is_paused(&self) -> bool {
        self.inner.paused.get()
    }

    pub fn current(&self) -> Option<BaseItem> {
        self.inner.queue.borrow().current().cloned()
    }

    pub fn queue(&self) -> std::cell::Ref<'_, Queue> {
        self.inner.queue.borrow()
    }

    pub fn repeat(&self) -> Repeat {
        self.inner.repeat.get()
    }

    pub fn position(&self) -> Option<f64> {
        self.inner.player.position()
    }

    pub fn duration(&self) -> Option<f64> {
        self.inner.player.duration()
    }

    pub fn client(&self) -> &Arc<EmbyClient> {
        &self.inner.client
    }

    /// Plays `items` from `start` (optionally shuffled), replacing the queue.
    pub fn play(&self, items: Vec<BaseItem>, start: usize, shuffle: bool) {
        if items.is_empty() {
            return;
        }
        let mut queue = Queue::new(items, start);
        if shuffle {
            queue.set_shuffle(true, seed());
        }
        self.inner.queue.replace(queue);
        self.activate();
        self.load_current();
    }

    fn activate(&self) {
        if self.inner.active.replace(true) {
            return;
        }
        let player = self.inner.player;
        // Never SVP, never blending: there's no video.
        let socket = crate::playback::svp_socket();
        for result in [
            player.set_svp(&socket, false),
            player.set_smooth_motion(false),
            player.set_audio_only(true),
        ] {
            if let Err(e) = result {
                tracing::warn!("{e:#}");
            }
        }
        let weak = Rc::downgrade(&self.inner);
        let timer = glib::timeout_add_local(PROGRESS_INTERVAL, move || match weak.upgrade() {
            Some(inner) => {
                MusicPlayer { inner }.report(Some("TimeUpdate"));
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        self.inner.timer.replace(Some(timer));
    }

    /// Stops music (also before video starts) and reports it.
    pub fn stop(&self) {
        if !self.inner.active.replace(false) {
            return;
        }
        if let Some(timer) = self.inner.timer.take() {
            timer.remove();
        }
        self.report_stopped(false);
        let player = self.inner.player;
        for result in [
            player.stop(),
            player.set_audio_only(false),
            player.set_loop_file(false),
        ] {
            if let Err(e) = result {
                tracing::warn!("{e:#}");
            }
        }
        self.inner.current.replace(None);
        self.inner.paused.set(false);
        self.changed();
    }

    pub fn toggle_pause(&self) {
        // The queue ran out: play its last track again.
        if self.inner.current.borrow().is_none() {
            return self.load_current();
        }
        if let Err(e) = self.inner.player.toggle_pause() {
            tracing::warn!("{e:#}");
        }
    }

    pub fn seek(&self, seconds: f64) {
        if let Err(e) = self.inner.player.seek_absolute(seconds) {
            tracing::warn!("{e:#}");
        }
    }

    pub fn next(&self) {
        let advanced = self.inner.queue.borrow_mut().advance(self.repeat());
        if advanced {
            self.report_stopped(false);
            self.load_current();
        }
    }

    pub fn previous(&self) {
        if self.position().is_some_and(|p| p > RESTART_THRESHOLD) {
            self.seek(0.0);
            return;
        }
        let moved = self.inner.queue.borrow_mut().back();
        if moved {
            self.report_stopped(false);
            self.load_current();
        } else {
            self.seek(0.0);
        }
    }

    pub fn jump(&self, position: usize) {
        let moved = self.inner.queue.borrow_mut().jump(position);
        if moved {
            self.report_stopped(false);
            self.load_current();
        }
    }

    pub fn play_next(&self, item: BaseItem) {
        if !self.is_active() {
            return self.play(vec![item], 0, false);
        }
        self.inner.queue.borrow_mut().play_next(item);
        self.requeue_next();
    }

    pub fn add(&self, item: BaseItem) {
        if !self.is_active() {
            return self.play(vec![item], 0, false);
        }
        self.inner.queue.borrow_mut().add(item);
        self.requeue_next();
    }

    pub fn remove(&self, position: usize) {
        if self.inner.queue.borrow_mut().remove(position) {
            self.requeue_next();
        }
    }

    pub fn shift(&self, position: usize, up: bool) {
        if self.inner.queue.borrow_mut().shift(position, up) {
            self.requeue_next();
        }
    }

    /// Moves the track at `from` to `to` (drag and drop).
    pub fn move_item(&self, from: usize, to: usize) {
        if self.inner.queue.borrow_mut().move_item(from, to) {
            self.requeue_next();
        }
    }

    pub fn clear_upcoming(&self) {
        self.inner.queue.borrow_mut().clear_upcoming();
        self.requeue_next();
    }

    pub fn set_shuffle(&self, on: bool) {
        if self.inner.queue.borrow().shuffled() == on {
            return;
        }
        self.inner.queue.borrow_mut().set_shuffle(on, seed());
        self.requeue_next();
    }

    pub fn cycle_repeat(&self) {
        let repeat = self.repeat().cycle();
        self.inner.repeat.set(repeat);
        if let Err(e) = self.inner.player.set_loop_file(repeat == Repeat::One) {
            tracing::warn!("{e:#}");
        }
        self.requeue_next();
    }

    /// Resolves the current track and starts it (replacing whatever plays).
    fn load_current(&self) {
        let Some(item) = self.current() else { return };
        let generation = self.inner.generation.get() + 1;
        self.inner.generation.set(generation);
        self.inner.next_queued.set(false);
        self.changed();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let track = this.resolve(item).await;
            if this.inner.generation.get() != generation || !this.is_active() {
                return;
            }
            match track {
                Ok(track) => {
                    // mpv's pause carries across files; a new track plays.
                    if let Err(e) = this
                        .inner
                        .player
                        .load(&track.url)
                        .and_then(|()| this.inner.player.set_pause(false))
                    {
                        tracing::warn!("{e:#}");
                        return;
                    }
                    this.inner.current.replace(Some(track));
                    this.report_playing();
                    this.queue_next();
                    this.changed();
                }
                Err(e) => tracing::warn!("couldn't play the track: {e:#}"),
            }
        });
    }

    /// After a queue edit: drop the preloaded next entry and queue the
    /// (possibly different) upcoming track instead.
    fn requeue_next(&self) {
        if self.inner.next_queued.replace(false)
            && let Err(e) = self.inner.player.playlist_remove(1)
        {
            tracing::debug!("{e:#}");
        }
        self.queue_next();
        self.changed();
    }

    /// Appends the upcoming track to mpv's playlist for a gapless switch.
    fn queue_next(&self) {
        if !self.is_active() || self.inner.current.borrow().is_none() {
            return;
        }
        let upcoming = {
            let queue = self.inner.queue.borrow();
            // Repeat-one loops the file itself; nothing to queue.
            match self.repeat() {
                Repeat::One => None,
                repeat => queue
                    .upcoming(repeat)
                    .and_then(|p| queue.item_at(p).cloned()),
            }
        };
        let Some(item) = upcoming else {
            self.inner.next_wanted.replace(None);
            return;
        };
        self.inner.next_wanted.replace(Some(item.id.clone()));
        let generation = self.inner.generation.get();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let id = item.id.clone();
            let track = this.resolve(item).await;
            if this.inner.generation.get() != generation
                || this.inner.next_queued.get()
                || this.inner.next_wanted.borrow().as_deref() != Some(id.as_str())
            {
                return;
            }
            match track.and_then(|t| this.inner.player.append(&t.url).map(|()| t)) {
                Ok(_) => this.inner.next_queued.set(true),
                Err(e) => tracing::warn!("couldn't queue the next track: {e:#}"),
            }
        });
    }

    async fn resolve(&self, item: BaseItem) -> Result<Track> {
        let client = self.inner.client.clone();
        let user_id = self.inner.user_id.clone();
        spawn_tokio(async move {
            let info = client
                .get_playback_info(&user_id, &item.id, &StreamRequest::default())
                .await?;
            let source = info
                .media_sources
                .first()
                .context("no playable source for this track")?;
            Ok(Track {
                url: client.direct_stream_url(&item.id, &source.id)?,
                media_source_id: source.id.clone(),
                play_session_id: info.play_session_id.clone(),
                item,
            })
        })
        .await
    }

    fn handle(&self, event: &PlayerEvent) {
        match event {
            // mpv moved on to the queued next track (gapless).
            PlayerEvent::PlaylistPos(1) if self.inner.next_queued.get() => {
                self.report_stopped(true);
                let advanced = self.inner.queue.borrow_mut().advance(self.repeat());
                if !advanced {
                    return;
                }
                // The finished track leaves mpv's playlist; the new one is
                // now entry 0.
                if let Err(e) = self.inner.player.playlist_remove(0) {
                    tracing::debug!("{e:#}");
                }
                self.inner.next_queued.set(false);
                let generation = self.inner.generation.get() + 1;
                self.inner.generation.set(generation);
                let item = self.current();
                let this = self.clone();
                glib::spawn_future_local(async move {
                    let Some(item) = item else { return };
                    // The stream is already playing; this only fetches the
                    // ids for reporting.
                    if let Ok(track) = this.resolve(item).await
                        && this.inner.generation.get() == generation
                    {
                        this.inner.current.replace(Some(track));
                        this.report_playing();
                    }
                    this.queue_next();
                    this.changed();
                });
                self.changed();
            }
            PlayerEvent::Pause(paused) => {
                self.inner.paused.set(*paused);
                self.report(Some(if *paused { "Pause" } else { "Unpause" }));
                self.changed();
            }
            // The last track ended with nothing queued.
            PlayerEvent::Finished(END_FILE_REASON_EOF) if !self.inner.next_queued.get() => {
                if self.repeat() == Repeat::One {
                    return;
                }
                self.report_stopped(false);
                self.inner.current.replace(None);
                self.inner.paused.set(true);
                self.changed();
            }
            _ => {}
        }
    }

    fn progress_request(&self, event: Option<&'static str>) -> Option<ProgressRequest> {
        let current = self.inner.current.borrow();
        let track = current.as_ref()?;
        Some(ProgressRequest {
            item_id: track.item.id.clone(),
            media_source_id: track.media_source_id.clone(),
            play_session_id: track.play_session_id.clone(),
            position_ticks: (self.position().unwrap_or(0.0) * TICKS_PER_SECOND) as i64,
            is_paused: self.inner.player.is_paused(),
            is_muted: self.inner.player.is_muted(),
            volume_level: self.inner.player.volume().round() as i32,
            can_seek: true,
            play_method: "DirectStream",
            audio_stream_index: None,
            subtitle_stream_index: None,
            event_name: event,
        })
    }

    fn report_playing(&self) {
        if let Some(request) = self.progress_request(None) {
            let client = self.inner.client.clone();
            spawn_detached(async move {
                if let Err(e) = client.report_playing(&request).await {
                    tracing::debug!("playing report failed: {e:#}");
                }
            });
        }
    }

    fn report(&self, event: Option<&'static str>) {
        if !self.is_active() {
            return;
        }
        if let Some(request) = self.progress_request(event) {
            let client = self.inner.client.clone();
            spawn_detached(async move {
                if let Err(e) = client.report_progress(&request).await {
                    tracing::debug!("progress report failed: {e:#}");
                }
            });
        }
    }

    /// `finished`: the track played to its end (a gapless switch).
    fn report_stopped(&self, finished: bool) {
        let Some(track) = self.inner.current.borrow().clone() else {
            return;
        };
        // At a gapless switch mpv is already on the next file, so a
        // finished track reports its full length.
        let position = track
            .item
            .run_time_ticks
            .filter(|_| finished)
            .unwrap_or_else(|| (self.position().unwrap_or(0.0) * TICKS_PER_SECOND) as i64);
        let request = StoppedRequest {
            item_id: track.item.id.clone(),
            media_source_id: track.media_source_id.clone(),
            play_session_id: track.play_session_id.clone(),
            position_ticks: position,
        };
        let client = self.inner.client.clone();
        spawn_detached(async move {
            if let Err(e) = client.report_stopped(&request).await {
                tracing::debug!("stopped report failed: {e:#}");
            }
        });
    }
}

impl Drop for Inner {
    /// Logging out drops the Ui and its player: the music stops with it.
    fn drop(&mut self) {
        if self.active.get() {
            if let Some(timer) = self.timer.take() {
                timer.remove();
            }
            let _ = self.player.stop();
            let _ = self.player.set_audio_only(false);
            let _ = self.player.set_loop_file(false);
        }
    }
}

fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
}
