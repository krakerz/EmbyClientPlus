//! The browse UI: one `Ui` per logged-in session, owning the navigation
//! stack that Home, Library, Series, Details, Search and the player live in.

mod card;
mod details;
mod home;
mod image_disk_cache;
mod images;
mod library;
mod library_page;
pub mod login;
pub mod player_page;
mod preferences;
mod rows;
mod search;
mod series;
pub mod window;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::glib;

use crate::config::Settings;
use crate::emby::EmbyClient;
use crate::emby::models::BaseItem;
use crate::playback::{PlaybackSession, Quality};
use crate::player::Player;
use player_page::{Handlers, PlayerPage};

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
    toasts: adw::ToastOverlay,
    player: Player,
    player_page: PlayerPage,
    playback: RefCell<Option<Rc<PlaybackSession>>>,
    /// Bumped whenever playback stops, so pages that show watch state know
    /// to reload when they come back into view.
    playback_generation: Cell<u64>,
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
        let ui = Ui {
            inner: Rc::new(Inner {
                session,
                nav: adw::NavigationView::new(),
                toasts,
                player,
                player_page,
                playback: RefCell::new(None),
                playback_generation: Cell::new(0),
                on_logout: Box::new(on_logout),
            }),
        };
        ui.inner.nav.add(&home::page(&ui));
        ui
    }

    pub fn widget(&self) -> &adw::NavigationView {
        &self.inner.nav
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
        self.inner.toasts.add_toast(adw::Toast::new(message));
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
        match item.item_type.as_str() {
            "Series" => self.push(&series::page(self, item)),
            "Season" => match &item.series_id {
                Some(series_id) => {
                    let series = BaseItem {
                        id: series_id.clone(),
                        name: item.series_name.clone().unwrap_or_default(),
                        item_type: "Series".into(),
                        ..Default::default()
                    };
                    self.push(&series::page(self, &series));
                }
                None => self.push(&library_page::page(self, item)),
            },
            _ if item.is_playable() => self.push(&details::page(self, item)),
            _ => self.push(&library_page::page(self, item)),
        }
    }

    pub fn open_search(&self) {
        self.push(&search::page(self));
    }

    pub fn playback_generation(&self) -> u64 {
        self.inner.playback_generation.get()
    }

    /// Plays `item` from `start_ticks` in the player page.
    pub fn play(&self, item: &BaseItem, start_ticks: i64) {
        // The current session's quality carries over to the next episode;
        // otherwise the configured default applies.
        let quality = self
            .inner
            .playback
            .borrow()
            .as_ref()
            .map(|session| session.quality)
            .unwrap_or_else(|| {
                Quality::from_mbps(
                    Settings::load()
                        .unwrap_or_default()
                        .playback
                        .bitrate_cap_mbps,
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

    /// Stops playback and waits briefly for Emby to get the final
    /// position, for window close.
    pub fn stop_playback_and_wait(&self) {
        if let Some(session) = self.inner.playback.take() {
            session.stop_and_wait();
        }
    }

    /// Stops the current playback session, if any, and reports it.
    pub fn stop_playback(&self) {
        if let Some(session) = self.inner.playback.take() {
            session.stop();
            let generation = &self.inner.playback_generation;
            generation.set(generation.get() + 1);
        }
    }
}

/// Re-runs `reload` whenever `page` is shown again after a playback ended,
/// so resume positions and watched marks stay current.
fn reload_after_playback(ui: &Ui, page: &adw::NavigationPage, reload: impl Fn() + 'static) {
    let seen = Cell::new(ui.playback_generation());
    let ui = ui.downgrade();
    page.connect_showing(move |_| {
        if let Some(ui) = ui.upgrade() {
            let generation = ui.playback_generation();
            if seen.replace(generation) != generation {
                reload();
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
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(header);
    toolbar.set_content(Some(&scrolled));
    let page = adw::NavigationPage::new(&toolbar, title);
    if let Some(tag) = tag {
        page.set_tag(Some(tag));
    }
    page
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
