//! The browse UI: one `Ui` per logged-in session, owning the navigation
//! stack that Home, Library, Series, Details, Search and the player live in.

mod album;
mod card;
mod category_tile;
mod details;
mod favorites;
mod gamepad;
mod hero;
mod home;
pub mod icons;
mod image_disk_cache;
pub(crate) mod images;
mod legend;
mod library;
mod library_page;
pub mod login;
mod marquee;
mod music;
pub mod player_page;
mod playlists;
mod preferences;
mod rows;
mod scrub_preview;
mod search;
mod series;
mod updates;
pub mod window;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::config::Settings;
use crate::emby::EmbyClient;
use crate::emby::models::BaseItem;
use crate::playback::{PlaybackSession, Quality};
use crate::player::Player;
use player_page::{Handlers, PlayerPage};

/// Songs in an Instant Mix.
const INSTANT_MIX_SIZE: usize = 100;

/// How long notices stay on screen.
pub const TOAST_SECONDS: u32 = 3;

/// An authenticated connection to the server, shared by every page.
#[derive(Clone)]
pub struct Session {
    pub client: Arc<EmbyClient>,
    pub user_id: String,
}

#[derive(Clone)]
pub struct Ui {
    inner: Rc<Inner>,
}

struct Inner {
    session: Session,
    nav: adw::NavigationView,
    /// Holds `nav` plus the music player's bar and Now Playing sheet.
    root: adw::BottomSheet,
    music: crate::music::MusicPlayer,
    music_panel: RefCell<Option<music::MusicPanel>>,
    toasts: adw::ToastOverlay,
    player: Player,
    player_page: PlayerPage,
    playback: RefCell<Option<Rc<PlaybackSession>>>,
    /// Bumped whenever watch state may have changed (playback stopped, an
    /// item marked watched or favourite), so pages showing it reload.
    data_generation: Cell<u64>,
    /// Each page's reload hook, so the visible one refreshes at once.
    reloaders: RefCell<Vec<Reloader>>,
    /// A web link (trailer) is playing outside any Emby session.
    playing_link: Cell<bool>,
    /// Reveals and focuses Home's search box.
    search_starter: RefCell<Option<Rc<dyn Fn()>>>,
    on_logout: Box<dyn Fn(bool)>,
}

impl Ui {
    /// `on_logout(expired)` runs when the user logs out, or with `true`
    /// when the server rejects the stored token.
    pub fn new(
        session: Session,
        toasts: adw::ToastOverlay,
        player: Player,
        player_page: PlayerPage,
        on_logout: impl Fn(bool) + 'static,
    ) -> Self {
        let music =
            crate::music::MusicPlayer::new(session.client.clone(), session.user_id.clone(), player);
        let ui = Ui {
            inner: Rc::new(Inner {
                session,
                nav: adw::NavigationView::new(),
                root: adw::BottomSheet::new(),
                music,
                music_panel: RefCell::new(None),
                toasts,
                player,
                player_page,
                playback: RefCell::new(None),
                data_generation: Cell::new(0),
                reloaders: RefCell::new(Vec::new()),
                playing_link: Cell::new(false),
                search_starter: RefCell::new(None),
                on_logout: Box::new(on_logout),
            }),
        };
        ui.inner.nav.add(&home::page(&ui));

        // `nav.home` is reachable from every page's header (see `add_home_button`).
        let actions = gio::SimpleActionGroup::new();
        let home = gio::SimpleAction::new("home", None);
        let nav = ui.inner.nav.downgrade();
        home.connect_activate(move |_, _| {
            if let Some(nav) = nav.upgrade() {
                nav.pop_to_tag("home");
            }
        });
        actions.add_action(&home);
        ui.inner.nav.insert_action_group("nav", Some(&actions));
        ui.inner.root.set_content(Some(&ui.inner.nav));
        let panel = music::attach(&ui, &ui.inner.music, &ui.inner.root);
        ui.inner.music_panel.replace(Some(panel));
        ui
    }

    pub fn widget(&self) -> &adw::BottomSheet {
        &self.inner.root
    }

    pub fn music(&self) -> &crate::music::MusicPlayer {
        &self.inner.music
    }

    /// Closes the music panel if it's open; false when it wasn't.
    pub fn close_now_playing(&self) -> bool {
        match self.music_panel() {
            Some(panel) if panel.is_open() => {
                panel.close();
                true
            }
            _ => false,
        }
    }

    /// The music panel (Now Playing + queue), for the controller.
    pub fn music_panel(&self) -> Option<music::MusicPanel> {
        self.inner.music_panel.borrow().clone()
    }

    pub fn session(&self) -> &Session {
        &self.inner.session
    }

    pub fn client(&self) -> Arc<EmbyClient> {
        self.inner.session.client.clone()
    }

    pub fn user_id(&self) -> String {
        self.inner.session.user_id.clone()
    }

    pub fn toast(&self, message: &str) {
        self.inner.toasts.add_toast(
            adw::Toast::builder()
                .title(glib::markup_escape_text(message))
                .timeout(TOAST_SECONDS)
                .build(),
        );
    }

    /// Reports a failed request: an expired token logs out, anything else
    /// becomes a toast.
    pub fn report_error(&self, context: &str, error: &anyhow::Error) {
        tracing::warn!("{context}: {error:#}");
        if crate::emby::is_unauthorized(error) {
            (self.inner.on_logout)(true);
        } else {
            self.toast(&format!("{context}: {error}"));
        }
    }

    pub fn logout(&self) {
        (self.inner.on_logout)(false);
    }

    fn push(&self, page: &adw::NavigationPage) {
        self.inner.nav.push(page);
    }

    /// Opens whatever page fits the item: series overview, playable item
    /// details, or a folder/library listing.
    pub fn open(&self, item: &BaseItem) {
        // A song plays right away, with its album as the queue.
        if item.is_audio() {
            return self.play_song(item);
        }
        self.push(&self.page_for(item));
    }

    /// Like [`open`](Self::open), but swaps out the current page, so
    /// stepping episode to episode doesn't pile up the back stack.
    pub fn open_replacing(&self, item: &BaseItem) {
        let nav = &self.inner.nav;
        let stack = nav.navigation_stack();
        let mut pages: Vec<adw::NavigationPage> = (0..stack.n_items())
            .filter_map(|i| stack.item(i).and_downcast())
            .collect();
        if pages.len() < 2 {
            return self.open(item);
        }
        pages.pop();
        pages.push(self.page_for(item));
        nav.replace(&pages);
    }

    fn page_for(&self, item: &BaseItem) -> adw::NavigationPage {
        match item.item_type.as_str() {
            "Series" => series::page(self, item),
            "Person" => library_page::person(self, item),
            "MusicAlbum" => album::page(self, item),
            "MusicArtist" => library_page::artist(self, item),
            "Playlist" => playlists::page(self, item),
            "Season" => match &item.series_id {
                Some(series_id) => {
                    let series = BaseItem {
                        id: series_id.clone(),
                        name: item.series_name.clone().unwrap_or_default(),
                        item_type: "Series".into(),
                        ..Default::default()
                    };
                    series::page(self, &series)
                }
                None => library_page::page(self, item),
            },
            _ if item.is_playable() => details::page(self, item),
            _ => library_page::page(self, item),
        }
    }

    /// Results for `term`, on their own page (refinable there).
    pub fn open_search(&self, term: &str) {
        self.push(&search::page(self, term));
    }

    /// Opens the search box in Home's header (going Home first).
    pub fn start_search(&self) {
        self.inner.nav.pop_to_tag("home");
        if let Some(start) = self.inner.search_starter.borrow().clone() {
            start();
        }
    }

    fn set_search_starter(&self, start: impl Fn() + 'static) {
        self.inner.search_starter.replace(Some(Rc::new(start)));
    }

    pub fn open_favorites(&self) {
        self.push(&favorites::page(self));
    }

    pub fn data_generation(&self) -> u64 {
        self.inner.data_generation.get()
    }

    /// Watch state changed: reload the visible page now; the others reload
    /// when they're shown again.
    pub fn data_changed(&self) {
        let generation = self.inner.data_generation.get() + 1;
        self.inner.data_generation.set(generation);
        let visible = self.inner.nav.visible_page();
        let reloaders: Vec<Reloader> = {
            let mut list = self.inner.reloaders.borrow_mut();
            list.retain(|r| r.page.upgrade().is_some());
            list.clone()
        };
        for reloader in reloaders {
            if reloader.page.upgrade() == visible {
                reloader.seen.set(generation);
                (reloader.reload)();
            }
        }
    }

    /// Marks `item` watched/unwatched on the server, then refreshes.
    pub fn set_played(&self, item: &BaseItem, played: bool) {
        let (client, user_id, id) = (self.client(), self.user_id(), item.id.clone());
        self.update_item(
            async move { client.set_played(&user_id, &id, played).await },
            if played {
                "Marked as watched"
            } else {
                "Marked as unwatched"
            },
        );
    }

    pub fn set_favorite(&self, item: &BaseItem, favorite: bool) {
        let (client, user_id, id) = (self.client(), self.user_id(), item.id.clone());
        self.update_item(
            async move { client.set_favorite(&user_id, &id, favorite).await },
            if favorite {
                "Added to favourites"
            } else {
                "Removed from favourites"
            },
        );
    }

    fn update_item(
        &self,
        request: impl std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
        done: &'static str,
    ) {
        let ui = self.clone();
        glib::spawn_future_local(async move {
            match crate::runtime::spawn_tokio(request).await {
                Ok(()) => {
                    ui.toast(done);
                    ui.data_changed();
                }
                Err(e) => ui.report_error("Couldn't update the item", &e),
            }
        });
    }

    /// Plays `items` from `start` in the music player (never the video
    /// player: no SVP, browsing carries on).
    pub fn play_music(&self, items: Vec<BaseItem>, start: usize, shuffle: bool) {
        self.stop_playback();
        if self.player_visible() {
            self.inner.nav.pop();
        }
        self.inner.music.play(items, start, shuffle);
    }

    /// Plays everything `item` stands for (album, artist, playlist).
    pub fn play_collection(&self, item: &BaseItem, shuffle: bool) {
        self.with_tracks(item, move |ui, tracks| ui.play_music(tracks, 0, shuffle));
    }

    /// Queues `item`'s tracks after the current one (`next`) or at the end.
    pub fn queue_music(&self, item: &BaseItem, next: bool) {
        let name = item.name.clone();
        self.with_tracks(item, move |ui, tracks| {
            let music = ui.music();
            if !music.is_active() {
                return ui.play_music(tracks, 0, false);
            }
            if next {
                // Each goes right after the current track: insert backwards.
                for track in tracks.into_iter().rev() {
                    music.play_next(track);
                }
            } else {
                for track in tracks {
                    music.add(track);
                }
            }
            ui.toast(&if next {
                format!("{name} plays next")
            } else {
                format!("Added {name} to the queue")
            });
        });
    }

    /// Plays Emby's Instant Mix of songs like `item`.
    pub fn play_instant_mix(&self, item: &BaseItem) {
        let ui = self.clone();
        let id = item.id.clone();
        glib::spawn_future_local(async move {
            let (client, user_id) = (ui.client(), ui.user_id());
            let mix = crate::runtime::spawn_tokio(async move {
                client.instant_mix(&user_id, &id, INSTANT_MIX_SIZE).await
            })
            .await;
            match mix {
                Ok(tracks) if tracks.is_empty() => ui.toast("No Instant Mix for this one"),
                Ok(tracks) => ui.play_music(tracks, 0, false),
                Err(e) => ui.report_error("Couldn't make an Instant Mix", &e),
            }
        });
    }

    pub fn add_to_playlist(&self, item: &BaseItem) {
        playlists::add_dialog(self, item);
    }

    fn with_tracks(&self, item: &BaseItem, then: impl FnOnce(&Ui, Vec<BaseItem>) + 'static) {
        let ui = self.clone();
        let item = item.clone();
        glib::spawn_future_local(async move {
            let (client, user_id) = (ui.client(), ui.user_id());
            let tracks = crate::runtime::spawn_tokio(async move {
                playlists::tracks_of(&client, &user_id, &item).await
            })
            .await;
            match tracks {
                Ok(tracks) if tracks.is_empty() => ui.toast("Nothing to play here"),
                Ok(tracks) => then(&ui, tracks),
                Err(e) => ui.report_error("Couldn't load the tracks", &e),
            }
        });
    }

    /// Plays one song with the rest of its album queued around it.
    fn play_song(&self, song: &BaseItem) {
        let Some(album_id) = song.album_id.clone() else {
            return self.play_music(vec![song.clone()], 0, false);
        };
        let ui = self.clone();
        let song = song.clone();
        glib::spawn_future_local(async move {
            let (client, user_id) = (ui.client(), ui.user_id());
            let tracks = crate::runtime::spawn_tokio(async move {
                client.album_tracks(&user_id, &album_id).await
            })
            .await;
            match tracks {
                Ok(tracks) if !tracks.is_empty() => {
                    let start = tracks.iter().position(|t| t.id == song.id).unwrap_or(0);
                    ui.play_music(tracks, start, false);
                }
                Ok(_) => ui.play_music(vec![song], 0, false),
                Err(e) => ui.report_error("Couldn't load the album", &e),
            }
        });
    }

    /// Plays `item` from `start_ticks` in the player page (songs go to the
    /// music player instead).
    pub fn play(&self, item: &BaseItem, start_ticks: i64) {
        if item.is_audio() {
            return self.play_song(item);
        }
        // The current session's quality carries over to the next episode;
        // otherwise the configured default applies.
        let quality = self
            .inner
            .playback
            .borrow()
            .as_ref()
            .map(|session| session.quality)
            .unwrap_or_else(|| {
                Quality::from_kbps(
                    Settings::load()
                        .unwrap_or_default()
                        .playback
                        .bitrate_cap_kbps(),
                )
            });
        self.play_with(item, start_ticks, quality, None);
    }

    fn play_with(
        &self,
        item: &BaseItem,
        start_ticks: i64,
        quality: Quality,
        audio_stream_index: Option<i32>,
    ) {
        self.inner.music.stop();
        self.stop_playback();
        let page = &self.inner.player_page;
        page.prepare(item, quality, self.player_handlers());
        if self.inner.nav.visible_page().as_ref() != Some(page.page()) {
            self.push(page.page());
        }

        let ui = self.clone();
        let item = item.clone();
        glib::spawn_future_local(async move {
            let started = PlaybackSession::start(
                ui.session().clone(),
                ui.inner.player,
                &item,
                start_ticks,
                quality,
                audio_stream_index,
            )
            .await;
            match started {
                Ok(session) => {
                    ui.inner.playback.replace(Some(session.clone()));
                    ui.inner.player_page.attach(session, &ui.client());
                }
                Err(e) if e.downcast_ref::<crate::remote::NeedsYtDlp>().is_some() => {
                    ui.open_in_browser(
                        e.downcast_ref::<crate::remote::NeedsYtDlp>()
                            .map(|n| n.0.clone()),
                    );
                    ui.inner.nav.pop();
                }
                Err(e) if quality != Quality::Original => {
                    tracing::warn!("transcode failed: {e:#}");
                    ui.toast("Transcoding failed, playing the original instead");
                    ui.play_with(&item, start_ticks, Quality::Original, None);
                }
                Err(e) => {
                    ui.report_error("Playback failed", &e);
                    ui.inner.nav.pop();
                }
            }
        });
    }

    /// Callbacks the player page uses to drive playback.
    fn player_handlers(&self) -> Handlers {
        let play = self.downgrade();
        let stopped = self.downgrade();
        let quality = self.downgrade();
        let toast = self.downgrade();
        Handlers {
            play: Box::new(move |item| {
                if let Some(ui) = play.upgrade() {
                    ui.play(item, item.resume_ticks());
                }
            }),
            change_quality: Box::new(move |chosen| {
                if let Some(ui) = quality.upgrade() {
                    ui.change_quality(chosen);
                }
            }),
            stopped: Box::new(move || {
                if let Some(ui) = stopped.upgrade() {
                    ui.stop_playback();
                }
            }),
            toast: Box::new(move |message| {
                if let Some(ui) = toast.upgrade() {
                    ui.toast(message);
                }
            }),
        }
    }

    /// Restarts the current item at the same spot and audio in `quality`.
    fn change_quality(&self, quality: Quality) {
        let Some(session) = self.inner.playback.borrow().clone() else {
            return;
        };
        let position = session.position_ticks();
        let audio = session.audio_stream_index();
        let item = session.item.clone();
        self.play_with(&item, position, quality, audio);
    }

    /// Plays a web link (a movie's YouTube trailer) without an Emby session.
    pub fn play_link(&self, title: &str, url: &str) {
        self.inner.music.stop();
        self.stop_playback();
        let item = BaseItem {
            name: title.to_string(),
            item_type: "Trailer".into(),
            ..Default::default()
        };
        let page = &self.inner.player_page;
        page.prepare(&item, Quality::Original, self.player_handlers());
        if self.inner.nav.visible_page().as_ref() != Some(page.page()) {
            self.push(page.page());
        }
        let ui = self.clone();
        let url = url.to_string();
        glib::spawn_future_local(async move {
            let link = url.clone();
            let resolved = crate::runtime::spawn_tokio(async move {
                let options =
                    crate::remote::Options::from_settings(&Settings::load().unwrap_or_default());
                tokio::task::spawn_blocking(move || crate::remote::resolve(&link, &options)).await?
            })
            .await;
            match resolved {
                Ok(stream) => match ui.inner.player.load_at(
                    &stream.url,
                    0.0,
                    // mpv shows a single added subtitle file by default.
                    &stream.subtitle.as_deref().into_iter().collect::<Vec<_>>(),
                    stream.audio.as_deref(),
                ) {
                    Ok(()) => ui.inner.playing_link.set(true),
                    Err(e) => {
                        ui.report_error("Playback failed", &e);
                        ui.inner.nav.pop();
                    }
                },
                Err(e) if e.downcast_ref::<crate::remote::NeedsYtDlp>().is_some() => {
                    ui.open_in_browser(Some(url));
                    ui.inner.nav.pop();
                }
                Err(e) => {
                    ui.report_error("Playback failed", &e);
                    ui.inner.nav.pop();
                }
            }
        });
    }

    /// Falls back to the browser for links mpv can't open without yt-dlp.
    fn open_in_browser(&self, url: Option<String>) {
        let Some(url) = url else { return };
        match gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>) {
            Ok(()) => self.toast("Opened in your browser; install yt-dlp to play trailers here"),
            Err(e) => self.toast(&format!("Couldn't open {url}: {e}")),
        }
    }

    /// Stops playback and waits briefly for Emby to get the final
    /// position, for window close.
    pub fn stop_playback_and_wait(&self) {
        self.inner.music.stop();
        if let Some(session) = self.inner.playback.take() {
            session.stop_and_wait();
        }
    }

    /// Stops the current playback session, if any, and reports it.
    pub fn stop_playback(&self) {
        if self.inner.playing_link.replace(false)
            && let Err(e) = self.inner.player.stop()
        {
            tracing::warn!("{e:#}");
        }
        if let Some(session) = self.inner.playback.take() {
            session.stop();
            let generation = &self.inner.data_generation;
            generation.set(generation.get() + 1);
        }
    }
}

#[derive(Clone)]
struct Reloader {
    page: glib::WeakRef<adw::NavigationPage>,
    reload: Rc<dyn Fn()>,
    /// The data generation this page last loaded at.
    seen: Rc<Cell<u64>>,
}

/// Like [`reload_on_change`], but also reloads every time `page` comes
/// back into view (Home: something new may have been added or watched).
fn reload_on_show(ui: &Ui, page: &adw::NavigationPage, reload: impl Fn() + 'static) {
    let reload = Rc::new(reload);
    let reloader = Reloader {
        page: page.downgrade(),
        reload: reload.clone(),
        seen: Rc::new(Cell::new(ui.data_generation())),
    };
    ui.inner.reloaders.borrow_mut().push(reloader);
    // The first showing is the page's initial load, done by its builder.
    let first = Cell::new(true);
    page.connect_showing(move |_| {
        if !first.replace(false) {
            reload();
        }
    });
}

/// Re-runs `reload` when watch state changed: immediately if `page` is
/// visible, otherwise when it's shown again (e.g. after playback).
fn reload_on_change(ui: &Ui, page: &adw::NavigationPage, reload: impl Fn() + 'static) {
    let reloader = Reloader {
        page: page.downgrade(),
        reload: Rc::new(reload),
        seen: Rc::new(Cell::new(ui.data_generation())),
    };
    ui.inner.reloaders.borrow_mut().push(reloader.clone());
    let ui = ui.downgrade();
    page.connect_showing(move |_| {
        if let Some(ui) = ui.upgrade() {
            let generation = ui.data_generation();
            if reloader.seen.replace(generation) != generation {
                (reloader.reload)();
            }
        }
    });
}

/// Weak handle for closures stored inside widgets the `Ui` owns.
#[derive(Clone)]
pub struct WeakUi(std::rc::Weak<Inner>);

impl WeakUi {
    pub fn upgrade(&self) -> Option<Ui> {
        self.0.upgrade().map(|inner| Ui { inner })
    }
}

impl Ui {
    pub fn downgrade(&self) -> WeakUi {
        WeakUi(Rc::downgrade(&self.inner))
    }
}

/// A page with a header bar over scrollable content.
fn scrolled_page(
    title: &str,
    tag: Option<&str>,
    header: &adw::HeaderBar,
    content: &impl IsA<gtk::Widget>,
) -> adw::NavigationPage {
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(content)
        .vexpand(true)
        .build();
    if tag != Some("home") {
        add_home_button(header);
    }
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(header);
    toolbar.set_content(Some(&scrolled));
    let page = adw::NavigationPage::new(&toolbar, title);
    if let Some(tag) = tag {
        page.set_tag(Some(tag));
    }
    page
}

const TAB_STEP_KEY: &str = "embyclientplus-tab-step";
const MENU_KEY: &str = "embyclientplus-card-menu";

type Hook<T> = Rc<dyn Fn(T)>;

/// Lets the controller's previous/next-tab buttons step through `page`'s
/// tabs or seasons.
fn set_tab_stepper(page: &adw::NavigationPage, step: impl Fn(bool) + 'static) {
    let hook: Hook<bool> = Rc::new(step);
    // SAFETY: this key is only ever stored and read as `Hook<bool>`.
    unsafe { page.set_data(TAB_STEP_KEY, hook) };
}

const FOCUS_HOOK_KEY: &str = "embyclientplus-focus-hook";

type FocusHook = Rc<dyn Fn() -> bool>;

/// Where the controller's cursor starts on `page` (e.g. a series' first
/// episode), instead of the first item; `focus` returns false while that
/// isn't there yet.
fn set_focus_hook(page: &adw::NavigationPage, focus: impl Fn() -> bool + 'static) {
    let hook: FocusHook = Rc::new(focus);
    // SAFETY: this key is only ever stored and read as `FocusHook`.
    unsafe { page.set_data(FOCUS_HOOK_KEY, hook) };
}

fn focus_hook(page: &adw::NavigationPage) -> Option<FocusHook> {
    // SAFETY: see `set_focus_hook`; cloned out immediately.
    unsafe {
        page.data::<FocusHook>(FOCUS_HOOK_KEY)
            .map(|hook| hook.as_ref().clone())
    }
}

const TAB_LABEL_KEY: &str = "embyclientplus-tab-label";

/// What LB/RB step through on `page`, for the controller legend.
fn set_tab_label(page: &adw::NavigationPage, label: &'static str) {
    // SAFETY: this key is only ever stored and read as `&'static str`.
    unsafe { page.set_data(TAB_LABEL_KEY, label) };
}

fn tab_label(page: &adw::NavigationPage) -> &'static str {
    // SAFETY: see `set_tab_label`; copied out immediately.
    unsafe {
        page.data::<&'static str>(TAB_LABEL_KEY)
            .map(|label| *label.as_ref())
            .unwrap_or("Tabs")
    }
}

fn tab_stepper(page: &adw::NavigationPage) -> Option<Hook<bool>> {
    // SAFETY: see `set_tab_stepper`; cloned out immediately.
    unsafe {
        page.data::<Hook<bool>>(TAB_STEP_KEY)
            .map(|hook| hook.as_ref().clone())
    }
}

/// Lets the controller's menu button open a card's context menu.
fn set_menu_opener(widget: &gtk::Widget, open: impl Fn(()) + 'static) {
    let hook: Hook<()> = Rc::new(open);
    // SAFETY: this key is only ever stored and read as `Hook<()>`.
    unsafe { widget.set_data(MENU_KEY, hook) };
}

fn menu_opener(widget: &gtk::Widget) -> Option<Hook<()>> {
    // SAFETY: see `set_menu_opener`; cloned out immediately.
    unsafe {
        widget
            .data::<Hook<()>>(MENU_KEY)
            .map(|hook| hook.as_ref().clone())
    }
}

impl Ui {
    pub fn nav(&self) -> &adw::NavigationView {
        &self.inner.nav
    }

    pub fn player_visible(&self) -> bool {
        self.inner.nav.visible_page().as_ref() == Some(self.inner.player_page.page())
    }

    pub fn player_page(&self) -> &PlayerPage {
        &self.inner.player_page
    }

    /// Steps the visible page's tabs/seasons, if it has any.
    pub fn step_tabs(&self, forward: bool) {
        if let Some(page) = self.inner.nav.visible_page()
            && let Some(step) = tab_stepper(&page)
        {
            step(forward);
        }
    }
}

/// Opens the context menu of the card `widget` belongs to, if any.
pub fn open_card_menu(widget: &gtk::Widget) -> bool {
    // The focus may be the grid cell around the card, or inside the card.
    let mut candidates = vec![widget.clone()];
    let mut child = widget.first_child();
    for _ in 0..2 {
        if let Some(c) = child {
            candidates.push(c.clone());
            child = c.first_child();
        }
    }
    let mut ancestor = widget.parent();
    while let Some(a) = ancestor {
        candidates.push(a.clone());
        ancestor = a.parent();
    }
    for candidate in candidates {
        if let Some(open) = menu_opener(&candidate) {
            open(());
            return true;
        }
    }
    false
}

/// A Home button beside the header's back button, so a deep trail of pages
/// doesn't have to be walked back one by one.
fn add_home_button(header: &adw::HeaderBar) {
    header.pack_start(
        &gtk::Button::builder()
            .icon_name(crate::ui::icons::HOME)
            .tooltip_text("Home")
            .action_name("nav.home")
            .build(),
    );
}

/// Height of the fan-art banner on series and details pages.
const BANNER_HEIGHT: i32 = 360;

/// Pins `picture` to exactly `width`×`height`. A `gtk::Picture` reports
/// its image's own size as its natural size, so without a cap the layout
/// grows it to whatever resolution the server sent.
fn fixed_picture(picture: &gtk::Picture, width: i32, height: i32) -> adw::Clamp {
    picture.set_size_request(width, height);
    let clamp = |orientation, size, child: &gtk::Widget| {
        adw::Clamp::builder()
            .orientation(orientation)
            .maximum_size(size)
            .tightening_threshold(size)
            .child(child)
            .build()
    };
    let horizontal = clamp(gtk::Orientation::Horizontal, width, picture.upcast_ref());
    clamp(gtk::Orientation::Vertical, height, horizontal.upcast_ref())
}

/// A dim type icon for `picture`, shown only while it has no image. Add it
/// as an overlay on the picture's frame.
fn placeholder_for(picture: &gtk::Picture, icon: &str) -> gtk::Image {
    let placeholder = gtk::Image::builder()
        .icon_name(icon)
        .pixel_size(48)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .can_target(false)
        .css_classes(["dim-label", "art-placeholder"])
        .visible(picture.paintable().is_none())
        .build();
    let weak = placeholder.downgrade();
    picture.connect_paintable_notify(move |picture| {
        if let Some(placeholder) = weak.upgrade() {
            placeholder.set_visible(picture.paintable().is_none());
        }
    });
    placeholder
}

/// The picture inside a [`fixed_picture`] wrapper.
fn fixed_picture_child(wrapper: &gtk::Widget) -> Option<gtk::Picture> {
    wrapper
        .downcast_ref::<adw::Clamp>()?
        .child()?
        .downcast_ref::<adw::Clamp>()?
        .child()?
        .downcast()
        .ok()
}

/// A full-width fan-art banner of fixed height.
fn banner() -> (gtk::Picture, adw::Clamp) {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .height_request(BANNER_HEIGHT)
        .hexpand(true)
        .build();
    let clamp = adw::Clamp::builder()
        .orientation(gtk::Orientation::Vertical)
        .maximum_size(BANNER_HEIGHT)
        .tightening_threshold(BANNER_HEIGHT)
        .child(&picture)
        .build();
    (picture, clamp)
}

/// Centered spinner shown while a page's first request is in flight.
fn loading() -> gtk::Widget {
    adw::Spinner::builder()
        .width_request(48)
        .height_request(48)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build()
        .upcast()
}

/// Removes every child of a box.
fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// "1 h 24 min" / "24 min" from Emby's 100ns ticks.
fn format_runtime(ticks: i64) -> String {
    let minutes = (ticks / crate::playback::TICKS_PER_SECOND / 60).max(0);
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// "1:02:03" / "12:34" playback timestamp from ticks.
fn format_timestamp(ticks: i64) -> String {
    let total = (ticks / crate::playback::TICKS_PER_SECOND).max(0);
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = crate::playback::TICKS_PER_SECOND;

    #[test]
    fn runtime_formatting() {
        assert_eq!(format_runtime(24 * 60 * SECOND), "24 min");
        assert_eq!(format_runtime(120 * 60 * SECOND), "2 h");
        assert_eq!(format_runtime(84 * 60 * SECOND), "1 h 24 min");
    }

    #[test]
    fn timestamp_formatting() {
        assert_eq!(format_timestamp(754 * SECOND), "12:34");
        assert_eq!(format_timestamp(3723 * SECOND), "1:02:03");
        assert_eq!(format_timestamp(-5), "0:00");
    }
}
