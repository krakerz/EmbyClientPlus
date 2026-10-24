//! Watch-progress sync to MyAnimeList, AniList, Kitsu and Trakt
//! (plans/2026-10-22-anime-tracking.md). The AniBridge mapping and the
//! resolver pick the entry; `tracker` queues and sends the updates.

pub mod anilist;
pub mod kitsu;
pub mod mal;
pub mod mapping;
pub mod markers;
pub mod oauth;
pub mod resolve;
pub mod rules;
pub mod tokens;
pub mod tracker;
pub mod trakt;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::LazyLock;

use anyhow::Result;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub const USER_AGENT: &str = concat!("EmbyClientPlus/", env!("CARGO_PKG_VERSION"));

/// One HTTP client for every service (Kitsu's Cloudflare wants a User-Agent).
pub fn http() -> &'static reqwest::Client {
    static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("tracking http client")
    });
    &HTTP
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceId {
    Mal,
    AniList,
    Kitsu,
    Trakt,
}

impl ServiceId {
    pub const ALL: [ServiceId; 4] = [
        ServiceId::Mal,
        ServiceId::AniList,
        ServiceId::Kitsu,
        ServiceId::Trakt,
    ];

    /// Stored in the queue and the keyring entry name.
    pub fn key(self) -> &'static str {
        match self {
            ServiceId::Mal => "mal",
            ServiceId::AniList => "anilist",
            ServiceId::Kitsu => "kitsu",
            ServiceId::Trakt => "trakt",
        }
    }

    pub fn from_key(key: &str) -> Option<ServiceId> {
        ServiceId::ALL.into_iter().find(|id| id.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            ServiceId::Mal => "MyAnimeList",
            ServiceId::AniList => "AniList",
            ServiceId::Kitsu => "Kitsu",
            ServiceId::Trakt => "Trakt",
        }
    }

    /// For the series page line.
    pub fn short(self) -> &'static str {
        match self {
            ServiceId::Mal => "MAL",
            other => other.label(),
        }
    }

    /// Logs in through the browser (loopback redirect): Desktop Mode only.
    pub fn needs_browser(self) -> bool {
        matches!(self, ServiceId::Mal | ServiceId::AniList)
    }

    /// Updated from the AniBridge match (Trakt goes by the item's own ids).
    pub fn uses_mapping(self) -> bool {
        self != ServiceId::Trakt
    }

    pub fn enabled_in(self, settings: &crate::config::TrackingSettings) -> bool {
        match self {
            ServiceId::Mal => settings.mal,
            ServiceId::AniList => settings.anilist,
            ServiceId::Kitsu => settings.kitsu,
            ServiceId::Trakt => settings.trakt,
        }
    }
}

/// The signed-in account, for the Preferences row.
#[derive(Debug, Clone)]
pub struct Account {
    pub name: String,
}

/// How a login is driven: MAL/AniList open the browser, Kitsu takes a
/// password (never stored), Trakt polls for a device code shown to the user.
pub enum LoginInput {
    Browser,
    Password { username: String, password: String },
    DeviceCode(trakt::DeviceCode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrobbleAction {
    Start,
    Pause,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Watched,
    Unwatched,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraktKind {
    Episode,
    Movie,
}

/// What Trakt identifies an item by: its own external ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraktItem {
    pub kind: TraktKind,
    pub ids: resolve::ShowIds,
}

impl TraktItem {
    /// `None` for anything but an episode or a movie, or without tvdb/tmdb/imdb.
    pub fn from_item(item: &crate::emby::models::BaseItem) -> Option<TraktItem> {
        let kind = match item.item_type.as_str() {
            "Episode" => TraktKind::Episode,
            "Movie" => TraktKind::Movie,
            _ => return None,
        };
        let ids = resolve::ShowIds::from_provider_ids(&item.provider_ids);
        (ids.tvdb.is_some() || ids.tmdb.is_some() || ids.imdb.is_some())
            .then_some(TraktItem { kind, ids })
    }
}

pub enum Update {
    /// MAL / AniList / Kitsu: the resolver's match (Kitsu maps the MAL id).
    Progress {
        target: resolve::Match,
        direction: Direction,
        movie: bool,
    },
    /// Trakt, live during playback. `progress` 0–100.
    Scrobble {
        item: TraktItem,
        action: ScrobbleAction,
        progress: f32,
    },
    /// Trakt: add to (unix seconds `watched_at`, or `released`) or remove from the history,
    /// every entry in one call. Adding leaves out what Trakt already lists
    /// as watched.
    HistoryBatch {
        direction: Direction,
        entries: Vec<HistoryEntry>,
    },
}

/// An episode of a show, for Trakt's watched list. Trakt numbers episodes
/// its own way, so the episode's own ids translate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowEpisode {
    pub show: resolve::ShowIds,
    /// The episode item's own tvdb/tmdb/imdb ids.
    pub ids: resolve::ShowIds,
    /// Episodes the file covers (1 unless it holds several).
    pub count: u32,
}

/// One episode or movie of a `HistoryBatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub item: TraktItem,
    /// `None`: Trakt's `"released"` (the episode's air date).
    pub watched_at: Option<i64>,
    /// `None` for movies (always added).
    pub episode: Option<ShowEpisode>,
}

/// What an update did. `Skipped` carries a reason for the log/toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Unchanged,
    Skipped(String),
}

/// The service rejected the token and it couldn't be renewed: the tokens
/// are deleted, the user must log in again.
#[derive(Debug)]
pub struct LoggedOut(pub ServiceId);

impl std::fmt::Display for LoggedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "logged out of {}", self.0.label())
    }
}

impl std::error::Error for LoggedOut {}

/// No connection (offline, DNS, timeout): keep the update queued.
pub fn is_network(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|e| e.is_connect() || e.is_timeout() || e.is_request())
    })
}

/// A list entry as the service has it now: its title and the user's own
/// entry (`None`: not on the list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInfo {
    pub title: String,
    pub entry: Option<rules::Entry>,
    /// Episode count (`None`: unknown).
    pub total: Option<u32>,
    /// The show is airing now (MAL `currently_airing`, AniList `RELEASING`,
    /// Kitsu `current`).
    pub airing: bool,
}

/// An entry to read for the sync markers: the id the match gives for the
/// service, and the AniList id of the same show (Kitsu looks it up when it
/// has no MAL mapping).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntryKey {
    pub id: u32,
    pub anilist: Option<u32>,
}

pub trait TrackingService: Send + Sync {
    fn id(&self) -> ServiceId;
    fn is_logged_in(&self) -> bool;
    fn account(&self) -> Option<Account>;
    /// AniList's 1-year token: when it runs out (unix seconds).
    fn expires_at(&self) -> Option<i64>;
    fn login(&self, input: LoginInput) -> BoxFuture<'_, Result<Account>>;
    fn logout(&self) -> BoxFuture<'_, Result<()>>;
    /// Renews the token when it's near expiry; a no-op otherwise.
    fn refresh(&self) -> BoxFuture<'_, Result<()>>;
    /// Updates the service doesn't handle return `Outcome::Unchanged`.
    fn update<'a>(&'a self, update: &'a Update) -> BoxFuture<'a, Result<Outcome>>;
    /// MAL / AniList / Kitsu (by MAL id): the entries with these ids, for the
    /// sync markers, keyed by `EntryKey::id`. Ids the service doesn't know
    /// are left out.
    fn entries<'a>(
        &'a self,
        _keys: &'a [EntryKey],
    ) -> BoxFuture<'a, Result<HashMap<u32, EntryInfo>>> {
        Box::pin(async { Ok(HashMap::new()) })
    }
    /// Trakt: the episodes watched on a show (`None`: Trakt doesn't know the
    /// show), from the session's copy when there is one.
    fn show_progress<'a>(
        &'a self,
        _ids: &'a resolve::ShowIds,
    ) -> BoxFuture<'a, Result<Option<trakt::ShowState>>> {
        Box::pin(async { Ok(None) })
    }
}
