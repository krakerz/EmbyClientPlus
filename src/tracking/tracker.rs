//! Queues and sends the watch updates to the tracking services.
//!
//! Every trigger (playback past the threshold, "Mark watched/unwatched")
//! writes a `tracking_queue` row first, so nothing is lost on a quit or
//! while offline; `flush` then sends the rows in order.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::markers::{self, Chip, Read, TraktRead};
use super::resolve::{self, EpisodeResolution, Match, Resolution, Route, ShowIds, Target};
use super::{
    Direction, EntryInfo, EntryKey, HistoryEntry, LoggedOut, Outcome, ScrobbleAction, ServiceId,
    ShowEpisode, TrackingService, TraktItem, Update, anilist, is_network, kitsu, mal, mapping,
    tokens, trakt,
};
use crate::config::Settings;
use crate::db::{Db, QueuedUpdate};
use crate::emby::models::BaseItem;

/// Emby marks an item played at 90 % of its runtime by default.
pub const PLAYED_PERCENT: f32 = 90.0;

/// Rows that keep failing are dropped after this many flushes.
const MAX_ATTEMPTS: i64 = 5;
const RETRY_EVERY: Duration = Duration::from_secs(15 * 60);
/// AniList's token lasts a year and can't be renewed.
const ANILIST_WARN_DAYS: i64 = 14;
/// Trakt allows one write per second.
const TRAKT_GAP: Duration = Duration::from_millis(1100);

/// The logged-in Emby session the updates are resolved against.
#[derive(Clone)]
pub struct SessionRef {
    pub client: Arc<crate::emby::EmbyClient>,
    pub user_id: String,
    pub server_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    /// Playback ended past the threshold; `progress` 0–100.
    Playback { progress: f32 },
    /// "Mark watched/unwatched" in the UI.
    Manual,
}

pub struct Tracker {
    services: Vec<Arc<dyn TrackingService>>,
    session: Mutex<Option<SessionRef>>,
    cache: Mutex<HashMap<String, Arc<Vec<EpisodeResolution>>>>,
    /// The sync markers' reads: (service, entry id) → the entry. Dropped
    /// after a flush that sent something, so our own updates show up.
    lists: Mutex<HashMap<(ServiceId, u32), EntryInfo>>,
    /// Series id → its external ids, for the Trakt marker.
    show_ids: Mutex<HashMap<String, ShowIds>>,
    /// One error toast per session per service.
    warned: Mutex<HashSet<ServiceId>>,
    flushing: tokio::sync::Mutex<()>,
    retry_started: AtomicBool,
}

pub fn get() -> &'static Tracker {
    static TRACKER: LazyLock<Tracker> = LazyLock::new(Tracker::new);
    &TRACKER
}

type Notifier = Box<dyn Fn(&str)>;

thread_local! {
    static NOTIFY: RefCell<Option<Notifier>> = const { RefCell::new(None) };
}

/// Toasts; called once from `Ui::new` on the GTK thread.
pub fn set_notifier(notify: impl Fn(&str) + 'static) {
    NOTIFY.with(|n| *n.borrow_mut() = Some(Box::new(notify)));
}

/// Shows a toast on the main window from any thread.
pub fn toast(text: &str) {
    notify(text.to_string());
}

/// Shows a toast from any thread.
fn notify(text: String) {
    gtk::glib::MainContext::default().invoke(move || {
        NOTIFY.with(|n| {
            if let Some(f) = &*n.borrow() {
                f(&text);
            }
        });
    });
}

/// What went wrong with one service's update.
enum Failure {
    LoggedOut,
    Network,
    Other,
}

/// A queue row waiting for its Trakt history entry to go out in a batch.
struct TraktRow {
    row: QueuedUpdate,
    name: String,
    state: RowState,
    entry: HistoryEntry,
    /// For an episode added as watched: its series and place in it, the
    /// anchor of the backfill.
    anchor: Option<(String, (u32, u32))>,
}

impl TraktRow {
    fn direction(&self) -> Direction {
        if self.row.watched {
            Direction::Watched
        } else {
            Direction::Unwatched
        }
    }
}

/// What a flush did with one queue row.
struct RowState {
    remaining: Vec<ServiceId>,
    /// The network is gone: save the row and end the flush.
    stop: bool,
    /// A service failed for another reason: counts as an attempt.
    failed: bool,
}

impl Tracker {
    fn new() -> Tracker {
        Tracker {
            services: vec![
                Arc::new(mal::Mal::new()),
                Arc::new(anilist::AniList::new()),
                Arc::new(kitsu::Kitsu::new()),
                Arc::new(trakt::Trakt::new()),
            ],
            session: Mutex::new(None),
            cache: Mutex::new(HashMap::new()),
            lists: Mutex::new(HashMap::new()),
            show_ids: Mutex::new(HashMap::new()),
            warned: Mutex::new(HashSet::new()),
            flushing: tokio::sync::Mutex::new(()),
            retry_started: AtomicBool::new(false),
        }
    }

    pub fn service(&self, id: ServiceId) -> Arc<dyn TrackingService> {
        self.services
            .iter()
            .find(|service| service.id() == id)
            .cloned()
            .expect("every service is registered")
    }

    /// Logged in and enabled in Settings.
    pub fn active(&self) -> Vec<ServiceId> {
        let settings = Settings::load().unwrap_or_default().tracking;
        ServiceId::ALL
            .into_iter()
            .filter(|&id| id.enabled_in(&settings) && self.service(id).is_logged_in())
            .collect()
    }

    fn session(&self) -> Option<SessionRef> {
        self.session.lock().ok()?.clone()
    }

    /// From `Ui::new`: remembers the session, refreshes tokens, warns about
    /// AniList's expiry, flushes the queue, starts the 15-min retry loop (once).
    pub fn set_session(&'static self, session: SessionRef) {
        if let Ok(mut slot) = self.session.lock() {
            *slot = Some(session);
        }
        if let Ok(mut warned) = self.warned.lock() {
            warned.clear();
        }
        crate::runtime::spawn_detached(async move {
            for id in self.active() {
                if let Err(e) = self.service(id).refresh().await {
                    self.report(id, &e);
                }
            }
            self.warn_anilist_expiry();
            self.flush().await;
        });
        if !self.retry_started.swap(true, Ordering::SeqCst) {
            crate::runtime::spawn_detached(async move {
                loop {
                    tokio::time::sleep(RETRY_EVERY).await;
                    self.flush().await;
                }
            });
        }
    }

    fn warn_anilist_expiry(&self) {
        if !self.active().contains(&ServiceId::AniList) {
            return;
        }
        let Some(at) = self.service(ServiceId::AniList).expires_at() else {
            return;
        };
        if at - tokens::now() <= ANILIST_WARN_DAYS * 86_400 {
            let (year, month, day) = crate::logging::civil_from_days(at.div_euclid(86_400));
            notify(format!(
                "AniList login expires on {year:04}-{month:02}-{day:02}: log in again in Preferences → Tracking"
            ));
        }
    }

    /// Trakt live scrobble (start/pause) — fire-and-forget.
    pub fn scrobble(&'static self, item: &BaseItem, action: ScrobbleAction, progress: f32) {
        if !self.active().contains(&ServiceId::Trakt) {
            return;
        }
        let Some(item) = TraktItem::from_item(item) else {
            return;
        };
        crate::runtime::spawn_detached(async move {
            let update = Update::Scrobble {
                item,
                action,
                progress,
            };
            if let Err(e) = self.service(ServiceId::Trakt).update(&update).await {
                if is_network(&e) {
                    tracing::debug!("Trakt scrobble {action:?}: {e:#}");
                } else {
                    tracing::warn!("Trakt scrobble {action:?} failed: {e:#}");
                }
            }
        });
    }

    /// End of playback: ≥ PLAYED_PERCENT → on_watch_change(Watched, Playback),
    /// else a Trakt scrobble Stop only (Trakt keeps it as paused progress).
    pub fn playback_stopped(&'static self, item: &BaseItem, progress: f32) {
        if progress >= PLAYED_PERCENT {
            self.on_watch_change(item, Direction::Watched, Source::Playback { progress });
        } else {
            self.scrobble(item, ScrobbleAction::Stop, progress);
        }
    }

    pub fn on_watch_change(&'static self, item: &BaseItem, direction: Direction, source: Source) {
        let bulk = matches!(item.item_type.as_str(), "Season" | "Series");
        if !bulk && !matches!(item.item_type.as_str(), "Episode" | "Movie") {
            return;
        }
        let settings = Settings::load().unwrap_or_default().tracking;
        if direction == Direction::Unwatched && !settings.sync_unwatched {
            return;
        }
        let mut services = self.active();
        if services.is_empty() {
            return;
        }
        let server_id = item
            .server_id
            .clone()
            .or_else(|| self.session().and_then(|s| s.server_id));
        if bulk {
            let Some(server_id) = server_id else {
                tracing::warn!("tracking: no server id, {} not synced", item.name);
                return;
            };
            let item = item.clone();
            crate::runtime::spawn_detached(async move {
                if let Err(e) = self
                    .queue_bulk(&item, direction, &services, &server_id)
                    .await
                {
                    tracing::warn!("tracking {}: {e:#}", item.name);
                }
            });
            return;
        }
        if let Source::Playback { progress } = source
            && let Some(at) = services.iter().position(|&id| id == ServiceId::Trakt)
        {
            // Trakt already got the live scrobble; a failed stop is queued
            // as a plain history entry instead.
            services.remove(at);
            if let Some(trakt_item) = TraktItem::from_item(item) {
                let item_id = item.id.clone();
                let server_id = server_id.clone();
                let anchor = backfill_anchor(item, true);
                crate::runtime::spawn_detached(async move {
                    let update = Update::Scrobble {
                        item: trakt_item,
                        action: ScrobbleAction::Stop,
                        progress,
                    };
                    let Err(e) = self.service(ServiceId::Trakt).update(&update).await else {
                        if let Some((series_id, upto)) = anchor {
                            self.backfill_after_stop(&series_id, upto).await;
                        }
                        return;
                    };
                    if matches!(self.report(ServiceId::Trakt, &e), Failure::LoggedOut) {
                        return;
                    }
                    if let Some(server_id) = server_id {
                        push(&server_id, &item_id, true, &[ServiceId::Trakt]);
                        self.flush().await;
                    }
                });
            }
        }
        if services.is_empty() {
            return;
        }
        let Some(server_id) = server_id else {
            tracing::warn!("tracking: no server id, {} not synced", item.name);
            return;
        };
        push(
            &server_id,
            &item.id,
            direction == Direction::Watched,
            &services,
        );
        crate::runtime::spawn_detached(async move { self.flush().await });
    }

    /// After a successful scrobble stop: adds the earlier played episodes
    /// Trakt lacks, in one history call.
    async fn backfill_after_stop(&self, series_id: &str, upto: (u32, u32)) {
        let Some(session) = self.session() else {
            return;
        };
        // Trakt allows one write per second.
        tokio::time::sleep(TRAKT_GAP).await;
        let entries = self
            .trakt_backfill(&session, series_id, upto, &mut HashMap::new())
            .await;
        if entries.is_empty() {
            return;
        }
        let update = Update::HistoryBatch {
            direction: Direction::Watched,
            entries,
        };
        match self.service(ServiceId::Trakt).update(&update).await {
            Ok(_) => {}
            Err(e) => {
                self.report(ServiceId::Trakt, &e);
            }
        }
    }

    /// A whole season or series marked (un)watched: one queue row per
    /// service entry (the highest episode it covers when watched, the
    /// lowest when unwatched), and every covered episode for Trakt.
    async fn queue_bulk(
        &'static self,
        item: &BaseItem,
        direction: Direction,
        services: &[ServiceId],
        server_id: &str,
    ) -> Result<()> {
        let (series_id, season, series_name) = if item.item_type == "Series" {
            (item.id.clone(), None, item.name.clone())
        } else {
            let series_id = item.series_id.clone().context("season without a series")?;
            let season = u32::try_from(item.index_number.context("season without a number")?)?;
            (
                series_id,
                Some(season),
                item.series_name.clone().unwrap_or_default(),
            )
        };
        // AniBridge only matters for the mapping services; Trakt goes by
        // the episodes' own ids and never waits for it.
        let resolved = if services.iter().any(|id| id.uses_mapping()) {
            self.series_status(series_id.clone()).await?
        } else {
            Arc::new(Vec::new())
        };
        let covered: Vec<&EpisodeResolution> = resolved
            .iter()
            .filter(|r| season.is_none_or(|season| r.season == season))
            .collect();
        let trakt_episodes: Vec<String> = if services.contains(&ServiceId::Trakt) {
            let session = self.session().context("no session")?;
            session
                .client
                .series_episodes(&series_id, &session.user_id)
                .await?
                .into_iter()
                .filter(|e| {
                    let number = e.parent_index_number.and_then(|n| u32::try_from(n).ok());
                    season.is_none_or(|season| number == Some(season))
                })
                .map(|e| e.id)
                .collect()
        } else {
            Vec::new()
        };
        let mut wanted: HashMap<String, Vec<ServiceId>> = HashMap::new();
        for &id in services {
            if id == ServiceId::Trakt {
                for episode_id in &trakt_episodes {
                    wanted.entry(episode_id.clone()).or_default().push(id);
                }
            } else {
                for pick in entry_updates(&covered, id, direction) {
                    wanted
                        .entry(pick.episode_id.to_string())
                        .or_default()
                        .push(id);
                }
            }
        }
        if services.iter().any(|id| id.uses_mapping()) {
            let unmatched = covered
                .iter()
                .filter(|r| matches!(r.resolution, Resolution::Skipped { silent: false, .. }))
                .count();
            if unmatched > 0 {
                notify(format!(
                    "Tracking: {unmatched} episode{} of {series_name} aren't matched, skipped",
                    if unmatched == 1 { "" } else { "s" }
                ));
            }
        }
        let mut order = trakt_episodes;
        let listed: HashSet<String> = order.iter().cloned().collect();
        order.extend(
            covered
                .iter()
                .filter(|r| !listed.contains(&r.episode_id))
                .map(|r| r.episode_id.clone()),
        );
        for episode_id in &order {
            if let Some(ids) = wanted.get(episode_id) {
                push(server_id, episode_id, direction == Direction::Watched, ids);
            }
        }
        self.flush().await;
        Ok(())
    }

    /// Sends the current server's queued rows in order.
    async fn flush(&'static self) {
        let _guard = self.flushing.lock().await;
        let Some(session) = self.session() else {
            return;
        };
        let Some(server_id) = session.server_id.clone() else {
            return;
        };
        let rows = tokio::task::spawn_blocking(move || {
            Db::open_default().and_then(|db| db.tracking_queue(&server_id))
        })
        .await;
        let rows = match rows {
            Ok(Ok(rows)) => rows,
            Ok(Err(e)) => {
                tracing::warn!("tracking queue unreadable: {e:#}");
                return;
            }
            Err(e) => {
                tracing::warn!("tracking queue task failed: {e}");
                return;
            }
        };
        let active = self.active();
        let mut blocked: HashSet<(String, ServiceId)> = HashSet::new();
        let mut shows: HashMap<String, ShowIds> = HashMap::new();
        let mut deferred: Vec<TraktRow> = Vec::new();
        let mut stopped = false;
        let sent = !rows.is_empty();
        for row in rows {
            let item = match session.client.item(&session.user_id, &row.item_id).await {
                Ok(item) => item,
                Err(e) if is_network(&e) => {
                    stopped = true;
                    break;
                }
                Err(e) => {
                    tracing::warn!("tracking: item {} unavailable, dropped: {e:#}", row.item_id);
                    drop_row(row.id);
                    continue;
                }
            };
            let mut state = RowState {
                remaining: row_services(&row, &active),
                stop: false,
                failed: false,
            };
            let entry = self
                .process_row(&session, &row, &item, &mut state, &mut blocked, &mut shows)
                .await;
            let stop = state.stop;
            match entry {
                // The Trakt part of a row goes out with its neighbours in
                // one call; the row is settled after that.
                Some(entry) => deferred.push(TraktRow {
                    anchor: backfill_anchor(&item, row.watched),
                    row,
                    name: item.name.clone(),
                    state,
                    entry,
                }),
                None => finish_row(&row, &item.name, &state),
            }
            if stop {
                stopped = true;
                break;
            }
        }
        let extra = if stopped {
            Vec::new()
        } else {
            self.backfill_for(&session, &deferred, &mut shows).await
        };
        self.send_trakt(deferred, extra, stopped).await;
        if sent {
            self.invalidate_lists();
        }
    }

    /// Sends the deferred Trakt history entries, one call per run of rows
    /// with the same direction (so a watch followed by an unwatch of the
    /// same episode keeps its order), then settles the rows. After a failure
    /// the later runs stay queued.
    /// The backfill entries go out with the first run that adds.
    async fn send_trakt(
        &self,
        mut deferred: Vec<TraktRow>,
        mut extra: Vec<HistoryEntry>,
        mut ended: bool,
    ) {
        let mut last: Option<Instant> = None;
        let mut at = 0;
        while at < deferred.len() {
            let direction = deferred[at].direction();
            let end = at
                + deferred[at..]
                    .iter()
                    .take_while(|r| r.direction() == direction)
                    .count();
            if !ended {
                if let Some(sent) = last {
                    tokio::time::sleep(TRAKT_GAP.saturating_sub(sent.elapsed())).await;
                }
                let mut entries: Vec<HistoryEntry> =
                    deferred[at..end].iter().map(|r| r.entry.clone()).collect();
                if direction == Direction::Watched {
                    entries.append(&mut extra);
                }
                let update = Update::HistoryBatch { direction, entries };
                let result = self.service(ServiceId::Trakt).update(&update).await;
                last = Some(Instant::now());
                match result {
                    Ok(outcome) => {
                        if let Outcome::Skipped(reason) = &outcome {
                            tracing::info!("Trakt skipped {} episodes: {reason}", end - at);
                        }
                        for r in &mut deferred[at..end] {
                            r.state.remaining.retain(|&id| id != ServiceId::Trakt);
                        }
                    }
                    Err(e) => {
                        ended = true;
                        match self.report(ServiceId::Trakt, &e) {
                            Failure::LoggedOut => {
                                for r in &mut deferred[at..] {
                                    r.state.remaining.retain(|&id| id != ServiceId::Trakt);
                                }
                            }
                            Failure::Network => {}
                            Failure::Other => {
                                for r in &mut deferred[at..end] {
                                    r.state.failed = true;
                                }
                            }
                        }
                    }
                }
            }
            at = end;
        }
        for r in &deferred {
            finish_row(&r.row, &r.name, &r.state);
        }
    }

    /// The earlier played episodes Trakt lacks, for each series with an
    /// anchor among `deferred` (up to its latest episode there).
    async fn backfill_for(
        &self,
        session: &SessionRef,
        deferred: &[TraktRow],
        shows: &mut HashMap<String, ShowIds>,
    ) -> Vec<HistoryEntry> {
        let mut anchors: BTreeMap<&str, (u32, u32)> = BTreeMap::new();
        for (series_id, at) in deferred.iter().filter_map(|r| r.anchor.as_ref()) {
            let latest = anchors.entry(series_id).or_insert(*at);
            *latest = (*latest).max(*at);
        }
        let mut entries = Vec::new();
        for (series_id, upto) in anchors {
            entries.extend(self.trakt_backfill(session, series_id, upto, shows).await);
        }
        entries
    }

    /// Earlier played episodes of one series that Trakt lacks; empty when
    /// anything needed can't be read (the next watch tries again).
    async fn trakt_backfill(
        &self,
        session: &SessionRef,
        series_id: &str,
        upto: (u32, u32),
        shows: &mut HashMap<String, ShowIds>,
    ) -> Vec<HistoryEntry> {
        let read = async {
            let show = series_ids(session, series_id, shows).await?;
            let Some(state) = self.service(ServiceId::Trakt).show_progress(&show).await? else {
                return Ok(Vec::new());
            };
            let episodes = session
                .client
                .series_episodes_played(series_id, &session.user_id)
                .await?;
            Ok::<_, anyhow::Error>(backfill_entries(upto, &episodes, &show, &state))
        };
        match read.await {
            Ok(entries) => entries,
            Err(e) => {
                tracing::debug!("Trakt backfill of {series_id} skipped: {e:#}");
                Vec::new()
            }
        }
    }

    async fn process_row(
        &self,
        session: &SessionRef,
        row: &QueuedUpdate,
        item: &BaseItem,
        state: &mut RowState,
        blocked: &mut HashSet<(String, ServiceId)>,
        shows: &mut HashMap<String, ShowIds>,
    ) -> Option<HistoryEntry> {
        let direction = if row.watched {
            Direction::Watched
        } else {
            Direction::Unwatched
        };
        let mapping_ids: Vec<ServiceId> = state
            .remaining
            .iter()
            .copied()
            .filter(|&id| id.uses_mapping() && !blocked.contains(&(row.item_id.clone(), id)))
            .collect();
        if !mapping_ids.is_empty() {
            match self.resolution_for(session, item).await {
                Ok(Some(found)) => match found.resolution {
                    Resolution::Matched(target) => {
                        let update = Update::Progress {
                            target,
                            direction,
                            movie: item.item_type == "Movie",
                        };
                        for id in mapping_ids {
                            if state.stop {
                                break;
                            }
                            let result = self.service(id).update(&update).await;
                            self.settle(id, &row.item_id, result, state, blocked);
                        }
                    }
                    Resolution::Skipped { silent, .. } => {
                        if !silent {
                            notify(unmatched_text(item));
                        }
                        state.remaining.retain(|id| !mapping_ids.contains(id));
                    }
                },
                Ok(None) => state.remaining.retain(|id| !mapping_ids.contains(id)),
                Err(e) if is_network(&e) => state.stop = true,
                Err(e) => {
                    tracing::warn!("tracking: resolving {} failed: {e:#}", item.name);
                    state.failed = true;
                }
            }
        }
        if state.stop || !state.remaining.contains(&ServiceId::Trakt) {
            return None;
        }
        let Some(trakt_item) = TraktItem::from_item(item) else {
            state.remaining.retain(|&id| id != ServiceId::Trakt);
            return None;
        };
        let episode = if item.item_type == "Episode" {
            match show_episode(session, item, shows).await {
                Ok(episode) => episode,
                Err(e) if is_network(&e) => {
                    state.stop = true;
                    return None;
                }
                Err(e) => {
                    tracing::warn!("tracking: show of {} unreadable: {e:#}", item.name);
                    state.failed = true;
                    return None;
                }
            }
        } else {
            None
        };
        Some(HistoryEntry {
            item: trakt_item,
            watched_at: Some(row.created_at),
            episode,
        })
    }

    /// Applies one service's result to the row.
    fn settle(
        &self,
        id: ServiceId,
        item_id: &str,
        result: Result<Outcome>,
        state: &mut RowState,
        blocked: &mut HashSet<(String, ServiceId)>,
    ) {
        match result {
            Ok(Outcome::Done | Outcome::Unchanged) => state.remaining.retain(|&s| s != id),
            Ok(Outcome::Skipped(reason)) => {
                tracing::info!("{} skipped {item_id}: {reason}", id.label());
                state.remaining.retain(|&s| s != id);
            }
            Err(e) => match self.report(id, &e) {
                Failure::LoggedOut => state.remaining.retain(|&s| s != id),
                Failure::Network => state.stop = true,
                Failure::Other => {
                    blocked.insert((item_id.to_string(), id));
                    state.failed = true;
                }
            },
        }
    }

    /// Logs and (once per session per service) toasts a failure.
    fn report(&self, id: ServiceId, error: &anyhow::Error) -> Failure {
        if error.chain().any(|c| c.is::<LoggedOut>()) {
            notify(format!(
                "Logged out of {}: log in again in Preferences → Tracking",
                id.label()
            ));
            return Failure::LoggedOut;
        }
        if is_network(error) {
            tracing::debug!("{}: {error:#}", id.label());
            return Failure::Network;
        }
        tracing::warn!("{} sync failed: {error:#}", id.label());
        let first = self.warned.lock().is_ok_and(|mut w| w.insert(id));
        if first {
            notify(format!("{} sync failed: {error}", id.label()));
        }
        Failure::Other
    }

    /// The mapping for one episode or movie: the cached series resolution
    /// when there is one, else resolved on its own.
    async fn resolution_for(
        &self,
        session: &SessionRef,
        item: &BaseItem,
    ) -> Result<Option<EpisodeResolution>> {
        if let Some(series_id) = &item.series_id {
            let cached = self
                .cache
                .lock()
                .ok()
                .and_then(|c| c.get(series_id).cloned());
            if let Some(found) =
                cached.and_then(|c| c.iter().find(|r| r.episode_id == item.id).cloned())
            {
                return Ok(Some(found));
            }
        }
        let index = mapping::index().await?;
        resolve::resolve_item(&session.client, &session.user_id, item, &index).await
    }

    /// Series page: resolves (cached per series id for this run) — needs the session.
    pub async fn series_status(
        &'static self,
        series_id: String,
    ) -> Result<Arc<Vec<EpisodeResolution>>> {
        let cached = self
            .cache
            .lock()
            .ok()
            .and_then(|c| c.get(&series_id).cloned());
        if let Some(cached) = cached {
            return Ok(cached);
        }
        let session = self.session().context("no session")?;
        let index = mapping::index().await?;
        let resolved = Arc::new(
            resolve::resolve_series(&session.client, &session.user_id, &series_id, &index).await?,
        );
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(series_id, resolved.clone());
        }
        Ok(resolved)
    }

    /// Drops the cache (after a logout/login).
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
        if let Ok(mut ids) = self.show_ids.lock() {
            ids.clear();
        }
        self.invalidate_lists();
    }

    /// The sync markers read the services again next time.
    fn invalidate_lists(&self) {
        if let Ok(mut lists) = self.lists.lock() {
            lists.clear();
        }
    }

    /// The sync markers of some episodes of one series (the items need
    /// their `ProviderIds`), by episode id: a chip per logged-in, enabled
    /// service. One read per distinct entry (AniList batches them), cached
    /// until the next flush that sent something. Trakt's chip never waits
    /// for AniBridge; only the MAL / AniList / Kitsu chips do.
    pub async fn episode_markers(
        &'static self,
        series_id: &str,
        series_name: &str,
        episodes: &[BaseItem],
    ) -> Result<HashMap<String, Vec<Chip>>> {
        let active = self.active();
        if active.is_empty() {
            return Ok(HashMap::new());
        }
        let session = self.session().context("no session")?;
        let queued = queued_pairs(&session).await;
        let resolved = if active.iter().any(|id| id.uses_mapping()) {
            match self.series_status(series_id.to_string()).await {
                Ok(resolved) => Some(resolved),
                Err(e) => {
                    tracing::debug!("sync markers: {series_id} not resolved: {e:#}");
                    None
                }
            }
        } else {
            None
        };
        let resolution_of = |episode_id: &str| {
            resolved
                .as_ref()
                .and_then(|all| all.iter().find(|r| r.episode_id == episode_id))
        };

        let mut reads: HashMap<ServiceId, std::result::Result<HashMap<u32, EntryInfo>, String>> =
            HashMap::new();
        for &id in active.iter().filter(|id| id.uses_mapping()) {
            let mut entries: BTreeMap<u32, EntryKey> = BTreeMap::new();
            for r in episodes.iter().filter_map(|e| resolution_of(&e.id)) {
                if let Resolution::Matched(found) = &r.resolution
                    && let Some(target) = target_of(id, found)
                {
                    entries.entry(target.id).or_insert(EntryKey {
                        id: target.id,
                        anilist: found.anilist.map(|a| a.id),
                    });
                }
            }
            if entries.is_empty() {
                // Nothing to read, but the unmatched episodes still get
                // their chip (the details page asks for one episode).
                reads.insert(id, Ok(HashMap::new()));
                continue;
            }
            let read = self
                .read_entries(id, entries.into_values().collect())
                .await
                .map_err(|e| self.read_failure(id, &e));
            reads.insert(id, read);
        }
        let trakt = if active.contains(&ServiceId::Trakt) {
            Some(self.trakt_read(&session, series_id).await)
        } else {
            None
        };

        let mut markers = HashMap::new();
        for episode in episodes {
            let mut chips = Vec::new();
            for id in markers::ORDER.into_iter().filter(|id| active.contains(id)) {
                let pending = queued.contains(&(episode.id.clone(), id));
                let chip = if id == ServiceId::Trakt {
                    let Some(outcome) = &trakt else {
                        continue;
                    };
                    let read = match outcome {
                        TraktOutcome::NoIds => TraktRead::NoIds,
                        TraktOutcome::Unknown => TraktRead::Unknown,
                        TraktOutcome::Failed(why) => TraktRead::Failed(why),
                        TraktOutcome::Show(state) => TraktRead::Show(state),
                    };
                    let number = |n: Option<i32>| n.and_then(|n| u32::try_from(n).ok());
                    markers::trakt_chip(
                        series_name,
                        number(episode.parent_index_number).unwrap_or(0),
                        number(episode.index_number).unwrap_or(0),
                        pending,
                        read,
                        &ShowIds::from_provider_ids(&episode.provider_ids),
                        episode_count(episode),
                    )
                } else {
                    let (Some(resolution), Some(read)) =
                        (resolution_of(&episode.id), reads.get(&id))
                    else {
                        continue;
                    };
                    let read = match read {
                        Ok(entries) => Read::Entries(entries),
                        Err(why) => Read::Failed(why),
                    };
                    markers::mapping_chip(id, &resolution.resolution, pending, read)
                };
                chips.push(chip);
            }
            markers.insert(episode.id.clone(), chips);
        }
        Ok(markers)
    }

    /// The entries of one mapping service: from the session's copy where
    /// there is one, the rest in one read.
    async fn read_entries(
        &self,
        id: ServiceId,
        keys: Vec<EntryKey>,
    ) -> Result<HashMap<u32, EntryInfo>> {
        let mut found = HashMap::new();
        let mut missing = Vec::new();
        if let Ok(lists) = self.lists.lock() {
            for key in keys {
                match lists.get(&(id, key.id)) {
                    Some(info) => {
                        found.insert(key.id, info.clone());
                    }
                    None => missing.push(key),
                }
            }
        }
        if !missing.is_empty() {
            let fetched = self.service(id).entries(&missing).await?;
            if let Ok(mut lists) = self.lists.lock() {
                for (entry, info) in &fetched {
                    lists.insert((id, *entry), info.clone());
                }
            }
            found.extend(fetched);
        }
        Ok(found)
    }

    /// Trakt's watched list for the series (its ids are read once).
    async fn trakt_read(&self, session: &SessionRef, series_id: &str) -> TraktOutcome {
        let cached = self
            .show_ids
            .lock()
            .ok()
            .and_then(|ids| ids.get(series_id).cloned());
        let ids = match cached {
            Some(ids) => ids,
            None => match session.client.item(&session.user_id, series_id).await {
                Ok(series) => {
                    let ids = ShowIds::from_provider_ids(&series.provider_ids);
                    if let Ok(mut known) = self.show_ids.lock() {
                        known.insert(series_id.to_string(), ids.clone());
                    }
                    ids
                }
                Err(e) => return TraktOutcome::Failed(self.read_failure(ServiceId::Trakt, &e)),
            },
        };
        if ids.tvdb.is_none() && ids.tmdb.is_none() && ids.imdb.is_none() {
            return TraktOutcome::NoIds;
        }
        match self.service(ServiceId::Trakt).show_progress(&ids).await {
            Ok(Some(state)) => TraktOutcome::Show(state),
            Ok(None) => TraktOutcome::Unknown,
            Err(e) => TraktOutcome::Failed(self.read_failure(ServiceId::Trakt, &e)),
        }
    }

    /// A short reason for a chip's tooltip; a lost login is reported as usual.
    fn read_failure(&self, id: ServiceId, error: &anyhow::Error) -> String {
        if error.chain().any(|c| c.is::<LoggedOut>()) {
            self.report(id, error);
            return "logged out".to_string();
        }
        if is_network(error) {
            return "offline".to_string();
        }
        tracing::debug!("{} marker read failed: {error:#}", id.label());
        error.to_string().chars().take(80).collect()
    }
}

/// How many episodes the item covers (a file can hold `S01E01-E02`).
fn episode_count(item: &BaseItem) -> u32 {
    match (item.index_number, item.index_number_end) {
        (Some(first), Some(last)) if last > first => u32::try_from(last - first + 1).unwrap_or(1),
        _ => 1,
    }
}

/// The series' external ids (read once per `shows`).
async fn series_ids(
    session: &SessionRef,
    series_id: &str,
    shows: &mut HashMap<String, ShowIds>,
) -> Result<ShowIds> {
    if let Some(ids) = shows.get(series_id) {
        return Ok(ids.clone());
    }
    let series = session.client.item(&session.user_id, series_id).await?;
    let ids = ShowIds::from_provider_ids(&series.provider_ids);
    shows.insert(series_id.to_string(), ids.clone());
    Ok(ids)
}

/// The series and (season, number) an episode that was watched anchors the
/// Trakt backfill at; `None` for movies, specials and unwatching.
fn backfill_anchor(item: &BaseItem, watched: bool) -> Option<(String, (u32, u32))> {
    if !watched || item.item_type != "Episode" {
        return None;
    }
    Some((item.series_id.clone()?, position(item)?))
}

/// (season, number) of a regular-season episode.
fn position(item: &BaseItem) -> Option<(u32, u32)> {
    let season = u32::try_from(item.parent_index_number?)
        .ok()
        .filter(|season| *season >= 1)?;
    Some((season, u32::try_from(item.index_number?).ok()?))
}

/// The episodes before `upto` that Emby marks played and Trakt doesn't list:
/// only ones Trakt's numbering can translate (by their ids), so a backfill
/// never adds a duplicate. Played at Emby's `LastPlayedDate`, else at the
/// release date. Only adds.
fn backfill_entries(
    upto: (u32, u32),
    episodes: &[BaseItem],
    show: &ShowIds,
    state: &trakt::ShowState,
) -> Vec<HistoryEntry> {
    episodes
        .iter()
        .filter(|e| position(e).is_some_and(|at| at < upto))
        .filter(|e| e.user_data.as_ref().is_some_and(|data| data.played))
        .filter_map(|e| {
            let item = TraktItem::from_item(e)?;
            let ids = ShowIds::from_provider_ids(&e.provider_ids);
            let count = episode_count(e);
            (state.episode_watched(&ids, count) == Some(false)).then(|| HistoryEntry {
                item,
                watched_at: e
                    .user_data
                    .as_ref()
                    .and_then(|data| data.last_played_date.as_deref())
                    .and_then(|text| gtk::glib::DateTime::from_iso8601(text, None).ok())
                    .map(|date| date.to_unix()),
                episode: Some(ShowEpisode {
                    show: show.clone(),
                    ids,
                    count,
                }),
            })
        })
        .collect()
}

/// The episode as Trakt's watched list wants it: the series' ids and the
/// episode's own (Trakt numbers differently, so ids translate it); `None`
/// without a series.
async fn show_episode(
    session: &SessionRef,
    item: &BaseItem,
    shows: &mut HashMap<String, ShowIds>,
) -> Result<Option<ShowEpisode>> {
    let Some(series_id) = item.series_id.as_deref() else {
        return Ok(None);
    };
    let show = series_ids(session, series_id, shows).await?;
    Ok(Some(ShowEpisode {
        show,
        ids: ShowIds::from_provider_ids(&item.provider_ids),
        count: episode_count(item),
    }))
}

/// Drops a settled queue row, or saves what is left of it (a row that
/// keeps failing is dropped after `MAX_ATTEMPTS`).
fn finish_row(row: &QueuedUpdate, name: &str, state: &RowState) {
    let attempts = row.attempts + i64::from(state.failed);
    if state.remaining.is_empty() {
        drop_row(row.id);
    } else if attempts >= MAX_ATTEMPTS {
        tracing::warn!("tracking: giving up on {name} after {attempts} attempts");
        drop_row(row.id);
    } else {
        let left = keys(&state.remaining);
        if let Err(e) =
            Db::open_default().and_then(|db| db.update_tracking(row.id, &left, attempts))
        {
            tracing::warn!("tracking queue update failed: {e:#}");
        }
    }
}

/// What reading a show's progress on Trakt came to.
enum TraktOutcome {
    NoIds,
    Unknown,
    Failed(String),
    Show(trakt::ShowState),
}

/// (episode id, service) of every queued update on the current server.
async fn queued_pairs(session: &SessionRef) -> HashSet<(String, ServiceId)> {
    let Some(server_id) = session.server_id.clone() else {
        return HashSet::new();
    };
    let rows = tokio::task::spawn_blocking(move || {
        Db::open_default().and_then(|db| db.tracking_queue(&server_id))
    })
    .await;
    let Ok(Ok(rows)) = rows else {
        return HashSet::new();
    };
    rows.iter()
        .flat_map(|row| {
            row.services
                .split(',')
                .filter_map(ServiceId::from_key)
                .map(|id| (row.item_id.clone(), id))
        })
        .collect()
}

fn keys(ids: &[ServiceId]) -> String {
    ids.iter().map(|id| id.key()).collect::<Vec<_>>().join(",")
}

/// The row's services that are still logged in and enabled.
fn row_services(row: &QueuedUpdate, active: &[ServiceId]) -> Vec<ServiceId> {
    row.services
        .split(',')
        .filter_map(ServiceId::from_key)
        .filter(|id| active.contains(id))
        .collect()
}

/// Writes a queue row now (sqlite on the caller's thread, so it survives
/// an immediate quit).
fn push(server_id: &str, item_id: &str, watched: bool, services: &[ServiceId]) {
    let result = Db::open_default().and_then(|db| {
        db.push_tracking(server_id, item_id, watched, &keys(services), tokens::now())
    });
    if let Err(e) = result {
        tracing::warn!("tracking queue write failed: {e:#}");
    }
}

fn drop_row(id: i64) {
    if let Err(e) = Db::open_default().and_then(|db| db.delete_tracking(id)) {
        tracing::warn!("tracking queue delete failed: {e:#}");
    }
}

fn unmatched_text(item: &BaseItem) -> String {
    if item.item_type == "Movie" {
        return format!("Tracking: {} isn't matched, skipped", item.name);
    }
    format!(
        "Tracking: {} S{}E{} isn't matched, skipped",
        item.series_name.as_deref().unwrap_or(&item.name),
        item.parent_index_number.unwrap_or_default(),
        item.index_number.unwrap_or_default(),
    )
}

/// What a service entry's target is for a match (Kitsu goes by MAL's).
pub(super) fn target_of(id: ServiceId, found: &Match) -> Option<Target> {
    match id {
        ServiceId::Mal | ServiceId::Kitsu => found.mal,
        ServiceId::AniList => found.anilist,
        ServiceId::Trakt => None,
    }
}

/// The episode that stands for one service entry in a bulk change.
#[derive(Debug, PartialEq, Eq)]
struct EntryUpdate<'a> {
    entry: u32,
    /// The episode number inside the entry.
    episode: u32,
    episode_id: &'a str,
}

/// Groups the matched episodes by the entry they land on: the highest
/// episode of each when marking watched (the list rules lift the progress
/// to it), the lowest when marking unwatched (progress becomes one less).
fn entry_updates<'a>(
    covered: &[&'a EpisodeResolution],
    id: ServiceId,
    direction: Direction,
) -> Vec<EntryUpdate<'a>> {
    let mut picks: BTreeMap<u32, EntryUpdate<'a>> = BTreeMap::new();
    for r in covered {
        let Resolution::Matched(found) = &r.resolution else {
            continue;
        };
        let Some(target) = target_of(id, found) else {
            continue;
        };
        let better = |old: &EntryUpdate| match direction {
            Direction::Watched => target.episode > old.episode,
            Direction::Unwatched => target.episode < old.episode,
        };
        if picks.get(&target.id).is_none_or(better) {
            picks.insert(
                target.id,
                EntryUpdate {
                    entry: target.id,
                    episode: target.episode,
                    episode_id: &r.episode_id,
                },
            );
        }
    }
    picks.into_values().collect()
}

fn route_text(route: Route) -> &'static str {
    match route {
        Route::SeasonAniDb | Route::SeriesIds | Route::ItemIds | Route::MovieLookup => "item IDs",
        Route::TvdbLookup => "TVDB lookup",
        Route::TmdbLookup => "TMDB lookup",
    }
}

/// "E5", "E13–E24" for sorted episode numbers, contiguous runs joined.
fn episode_runs(numbers: &[u32]) -> String {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &n in numbers {
        match runs.last_mut() {
            Some((_, end)) if n <= *end + 1 => *end = n.max(*end),
            _ => runs.push((n, n)),
        }
    }
    runs.iter()
        .map(|&(start, end)| {
            if start == end {
                format!("E{start}")
            } else {
                format!("E{start}–E{end}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// "Tracking: MAL ✓ · AniList ✓ (item IDs) · S3 E13–E24 not matched"; `None`
/// when no mapping service is active. Pure, tested.
pub fn status_line(resolved: &[EpisodeResolution], active: &[ServiceId]) -> Option<String> {
    let mapping: Vec<ServiceId> = active
        .iter()
        .copied()
        .filter(|id| id.uses_mapping())
        .collect();
    if mapping.is_empty() {
        return None;
    }
    let regular: Vec<&EpisodeResolution> = resolved.iter().filter(|r| r.season >= 1).collect();
    if regular.is_empty() {
        return None;
    }
    let matches: Vec<&Match> = regular
        .iter()
        .filter_map(|r| match &r.resolution {
            Resolution::Matched(found) => Some(found),
            Resolution::Skipped { .. } => None,
        })
        .collect();
    if matches.is_empty() {
        return Some("Tracking: not matched".to_string());
    }
    let hit: Vec<bool> = mapping
        .iter()
        .map(|&id| matches.iter().any(|found| target_of(id, found).is_some()))
        .collect();
    let route = matches
        .iter()
        .find(|found| mapping.iter().any(|&id| target_of(id, found).is_some()))
        .map(|found| route_text(found.route));
    let last_hit = hit.iter().rposition(|&hit| hit);
    let mut parts: Vec<String> = Vec::new();
    for (at, &id) in mapping.iter().enumerate() {
        parts.push(match (hit[at], route) {
            (true, Some(route)) if Some(at) == last_hit => {
                format!("{} ✓ ({route})", id.short())
            }
            (true, _) => format!("{} ✓", id.short()),
            (false, _) => format!("{} ✗", id.short()),
        });
    }
    let mut missing: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for r in &regular {
        if matches!(r.resolution, Resolution::Skipped { silent: false, .. }) {
            missing.entry(r.season).or_default().push(r.episode);
        }
    }
    for (season, mut numbers) in missing {
        numbers.sort_unstable();
        numbers.dedup();
        parts.push(format!("S{season} {} not matched", episode_runs(&numbers)));
    }
    let mut suspects: BTreeMap<u32, u32> = BTreeMap::new();
    for r in &regular {
        let suspect = match &r.resolution {
            Resolution::Matched(found) => found.suspect_season_anidb,
            Resolution::Skipped {
                suspect_season_anidb,
                ..
            } => *suspect_season_anidb,
        };
        if let Some(anidb) = suspect {
            suspects.entry(r.season).or_insert(anidb);
        }
    }
    for (season, anidb) in suspects {
        parts.push(format!("S{season} AniDB {anidb} ignored"));
    }
    Some(format!("Tracking: {}", parts.join(" · ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u32, episode: u32) -> Option<Target> {
        Some(Target {
            id,
            episode,
            extrapolated: false,
        })
    }

    fn matched(
        season: u32,
        episode: u32,
        mal: Option<Target>,
        anilist: Option<Target>,
    ) -> EpisodeResolution {
        EpisodeResolution {
            episode_id: format!("s{season}e{episode}"),
            season,
            episode,
            resolution: Resolution::Matched(Match {
                mal,
                anilist,
                route: Route::SeasonAniDb,
                suspect_season_anidb: None,
            }),
        }
    }

    fn skipped(season: u32, episode: u32, suspect: Option<u32>) -> EpisodeResolution {
        EpisodeResolution {
            episode_id: format!("s{season}e{episode}"),
            season,
            episode,
            resolution: Resolution::Skipped {
                silent: false,
                suspect_season_anidb: suspect,
            },
        }
    }

    /// Dr. Stone season 4: E1–12, E13–24 and E25–37 are three entries.
    fn dr_stone_four() -> Vec<EpisodeResolution> {
        (1..=37)
            .map(|episode| {
                let (mal, anilist, number) = match episode {
                    1..=12 => (57592, 172019, episode),
                    13..=24 => (61322, 189117, episode - 12),
                    _ => (62568, 199221, episode - 24),
                };
                matched(4, episode, t(mal, number), t(anilist, number))
            })
            .collect()
    }

    fn all_services() -> Vec<ServiceId> {
        ServiceId::ALL.to_vec()
    }

    #[test]
    fn dr_stone_marked_played_updates_three_entries() {
        let resolved = dr_stone_four();
        let covered: Vec<&EpisodeResolution> = resolved.iter().collect();
        for id in [ServiceId::Mal, ServiceId::AniList, ServiceId::Kitsu] {
            let picks = entry_updates(&covered, id, Direction::Watched);
            let episodes: Vec<u32> = picks.iter().map(|p| p.episode).collect();
            assert_eq!(episodes, [12, 12, 13], "{id:?}");
            let ids: Vec<&str> = picks.iter().map(|p| p.episode_id).collect();
            assert_eq!(ids, ["s4e12", "s4e24", "s4e37"], "{id:?}");
        }
        assert!(entry_updates(&covered, ServiceId::Trakt, Direction::Watched).is_empty());
    }

    #[test]
    fn dr_stone_marked_unplayed_uses_each_entrys_first_episode() {
        let resolved = dr_stone_four();
        let covered: Vec<&EpisodeResolution> = resolved.iter().collect();
        let picks = entry_updates(&covered, ServiceId::Mal, Direction::Unwatched);
        let ids: Vec<&str> = picks.iter().map(|p| p.episode_id).collect();
        assert_eq!(ids, ["s4e1", "s4e13", "s4e25"]);
        assert!(picks.iter().all(|p| p.episode == 1));
    }

    #[test]
    fn entries_without_a_target_for_the_service_are_left_out() {
        let resolved = [matched(1, 1, t(5, 1), None), skipped(1, 2, None)];
        let covered: Vec<&EpisodeResolution> = resolved.iter().collect();
        assert_eq!(
            entry_updates(&covered, ServiceId::Mal, Direction::Watched).len(),
            1
        );
        assert!(entry_updates(&covered, ServiceId::AniList, Direction::Watched).is_empty());
    }

    #[test]
    fn status_line_all_matched() {
        let resolved = vec![
            matched(1, 1, t(1, 1), t(2, 1)),
            matched(1, 2, t(1, 2), t(2, 2)),
        ];
        assert_eq!(
            status_line(&resolved, &all_services()).as_deref(),
            Some("Tracking: MAL ✓ · AniList ✓ · Kitsu ✓ (item IDs)")
        );
        assert_eq!(
            status_line(&resolved, &[ServiceId::Mal, ServiceId::AniList]).as_deref(),
            Some("Tracking: MAL ✓ · AniList ✓ (item IDs)")
        );
    }

    #[test]
    fn status_line_names_the_missing_part_of_a_split_season() {
        let mut resolved: Vec<EpisodeResolution> =
            (1..=12).map(|e| matched(3, e, t(1, e), t(2, e))).collect();
        resolved.extend((13..=24).map(|e| skipped(3, e, None)));
        resolved.push(skipped(3, 30, None));
        resolved.push(matched(0, 1, None, None));
        assert_eq!(
            status_line(&resolved, &[ServiceId::Mal, ServiceId::AniList]).as_deref(),
            Some("Tracking: MAL ✓ · AniList ✓ (item IDs) · S3 E13–E24, E30 not matched")
        );
    }

    #[test]
    fn status_line_flags_a_suspect_season_id_and_a_missing_service() {
        let mut found = matched(1, 1, t(1, 1), None);
        if let Resolution::Matched(m) = &mut found.resolution {
            m.suspect_season_anidb = Some(14087);
        }
        assert_eq!(
            status_line(&[found], &[ServiceId::Mal, ServiceId::AniList]).as_deref(),
            Some("Tracking: MAL ✓ (item IDs) · AniList ✗ · S1 AniDB 14087 ignored")
        );
    }

    #[test]
    fn status_line_nothing_matched_or_no_mapping_service() {
        let resolved = vec![skipped(1, 1, None)];
        assert_eq!(
            status_line(&resolved, &[ServiceId::Kitsu]).as_deref(),
            Some("Tracking: not matched")
        );
        assert_eq!(status_line(&resolved, &[ServiceId::Trakt]), None);
        assert_eq!(status_line(&resolved, &[]), None);
    }

    #[test]
    fn episode_runs_join_contiguous_numbers() {
        assert_eq!(episode_runs(&[5]), "E5");
        assert_eq!(episode_runs(&[1, 2, 3, 7]), "E1–E3, E7");
    }

    #[test]
    fn a_file_covering_several_episodes_counts_them() {
        let mut item = BaseItem {
            index_number: Some(3),
            ..Default::default()
        };
        assert_eq!(episode_count(&item), 1);
        item.index_number_end = Some(4);
        assert_eq!(episode_count(&item), 2);
        item.index_number_end = Some(3);
        assert_eq!(episode_count(&item), 1);
    }

    fn emby_episode(
        season: i32,
        number: i32,
        tvdb: u32,
        played: bool,
        date: Option<&str>,
    ) -> BaseItem {
        serde_json::from_value(serde_json::json!({
            "Id": format!("e{season}x{number}"),
            "Type": "Episode",
            "ParentIndexNumber": season,
            "IndexNumber": number,
            "ProviderIds": {"Tvdb": tvdb.to_string()},
            "UserData": {"Played": played, "LastPlayedDate": date},
        }))
        .unwrap()
    }

    #[test]
    fn backfill_adds_only_earlier_played_translatable_episodes_trakt_lacks() {
        let seasons = r#"[{"number":1,"episodes":[
            {"season":1,"number":1,"ids":{"tvdb":901}},
            {"season":1,"number":2,"ids":{"tvdb":902}},
            {"season":1,"number":3,"ids":{"tvdb":903}},
            {"season":1,"number":5,"ids":{"tvdb":905}},
            {"season":1,"number":7,"ids":{"tvdb":907}}]}]"#;
        let progress = r#"{"seasons":[{"number":1,"episodes":[{"number":3,"completed":true}]}]}"#;
        let state = trakt::ShowState {
            watched: Arc::new(trakt::watched_from_progress(
                &serde_json::from_str(progress).unwrap(),
            )),
            episodes: Arc::new(trakt::episode_map_from_seasons(
                &serde_json::from_str(seasons).unwrap(),
            )),
        };
        let episodes = [
            emby_episode(0, 1, 900, true, None),
            emby_episode(1, 1, 901, true, Some("2026-10-01T10:00:00.0000000Z")),
            emby_episode(1, 2, 902, true, None),
            // Trakt already has it.
            emby_episode(1, 3, 903, true, None),
            // Trakt's numbering can't translate it: skipped, never guessed.
            emby_episode(1, 4, 904, true, None),
            // Not played.
            emby_episode(1, 5, 905, false, None),
            // The anchor itself and later ones.
            emby_episode(1, 7, 907, true, None),
            emby_episode(1, 8, 908, true, None),
        ];
        let show = ShowIds {
            tvdb: Some(9),
            ..Default::default()
        };
        let entries = backfill_entries((1, 7), &episodes, &show, &state);
        let tvdb: Vec<Option<u32>> = entries.iter().map(|e| e.item.ids.tvdb).collect();
        assert_eq!(tvdb, [Some(901), Some(902)]);
        assert_eq!(entries[0].watched_at, Some(1_790_848_800));
        assert_eq!(entries[1].watched_at, None);
        assert_eq!(
            entries[0].episode.as_ref().map(|e| e.show.tvdb),
            Some(Some(9))
        );
    }

    #[test]
    fn backfill_anchors_only_watched_regular_episodes() {
        let item = emby_episode(2, 4, 1, true, None);
        let mut item = item;
        item.series_id = Some("s".into());
        assert_eq!(
            backfill_anchor(&item, true),
            Some(("s".to_string(), (2, 4)))
        );
        assert_eq!(backfill_anchor(&item, false), None);
        item.parent_index_number = Some(0);
        assert_eq!(backfill_anchor(&item, true), None);
    }
}
