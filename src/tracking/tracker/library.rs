//! Mirrors what is played on the Emby server into `overrides.db`
//! (`library_watched`) and sends what Trakt lacks.
//!
//! The first pass reads every played movie and episode, diffs them against
//! Trakt's watched lists and adds only what is missing. Later passes read
//! pages of the newest plays until they meet known rows, so plays from other
//! apps follow without rereading the library; a full read repeats weekly (and
//! catches items that stopped being played).

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};

use super::{
    BaseItem, Direction, Failure, HistoryEntry, Instant, Outcome, ServiceId, SessionRef,
    ShowEpisode, ShowIds, TRAKT_GAP, Tracker, TraktItem, is_network, last_played, show_episode,
    tokens,
};
use crate::config::Settings;
use crate::db::{Db, LibraryScope, PlayedRow, TraktState};
use crate::tracking::TraktKind;

/// Items per played listing page.
const PAGE: usize = 500;
/// A full read of the played list repeats after this long.
const FULL_SCAN_SECS: i64 = 7 * 86_400;
/// An automatic pass sends at most this many items ...
const AUTO_ROWS: usize = 300;
/// ... from at most this many series.
const AUTO_SERIES: usize = 20;
/// Ids per `items_by_ids` call.
const ITEM_CHUNK: usize = 100;
/// Fewer unplayed items than this are always removed; above it, a tenth of
/// the library is the limit.
const MIN_UNPLAYS_GUARD: usize = 25;
/// Entries per Trakt history call.
const SEND_CHUNK: usize = 300;

/// How much of the played list a pass reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scan {
    /// Newest plays until known rows appear (a full read once a week).
    Incremental,
    /// Newest plays until known rows appear, never upgraded to a full read
    /// (the quit sync has seconds, not minutes).
    Quick,
    /// Every page.
    Full,
}

/// How much a pass sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Caps {
    /// Bounded, so a background pass stays small.
    Automatic,
    /// Everything that is pending.
    Unlimited,
}

/// What one library pass did.
#[derive(Debug, Clone, Default)]
pub struct LibraryReport {
    /// Played items read from Emby.
    pub listed: usize,
    /// Of those, not known before.
    pub new: usize,
    /// Added to Trakt's history.
    pub sent: usize,
    /// Checked and already on Trakt.
    pub already: usize,
    /// Without ids Trakt could match.
    pub no_ids: usize,
    /// Plays removed from Trakt because the item is no longer played.
    pub removed: usize,
    /// Still pending afterwards.
    pub left: usize,
    /// Why the pass ended early (quota, offline, login lost ...).
    pub stopped: Option<String>,
}

impl LibraryReport {
    /// One sentence for the settings row and the log.
    pub fn summary(&self) -> String {
        let mut text = format!(
            "{} played items read ({} new), {} added to Trakt, {} already there",
            self.listed, self.new, self.sent, self.already
        );
        if self.no_ids > 0 {
            text.push_str(&format!(", {} without ids", self.no_ids));
        }
        if self.removed > 0 {
            text.push_str(&format!(", {} removed from Trakt", self.removed));
        }
        if self.left > 0 {
            text.push_str(&format!(", {} left for later", self.left));
        }
        if let Some(reason) = &self.stopped {
            text.push_str(&format!(". Stopped: {reason}"));
        }
        text
    }
}

/// What a pass actually reads: an incremental one becomes a full read when
/// the last one is missing or a week old.
fn effective_scan(scan: Scan, last_full: Option<i64>, now: i64) -> Scan {
    match scan {
        Scan::Incremental if last_full.is_none_or(|at| now - at > FULL_SCAN_SECS) => Scan::Full,
        other => other,
    }
}

/// A rebuilt server library lists few items as played; removing that many
/// plays from Trakt automatically would be a disaster, so a big batch waits
/// for the user.
fn too_many_unplays(unseen: usize, on_trakt: usize) -> bool {
    unseen > MIN_UNPLAYS_GUARD.max(on_trakt / 10)
}

/// The mirror row of a listed item.
fn played_row(item: &BaseItem) -> PlayedRow {
    PlayedRow {
        item_id: item.id.clone(),
        series_id: item.series_id.clone(),
        played_at: last_played(item),
    }
}

/// The rows an automatic pass handles: the first `AUTO_ROWS`, from at most
/// `AUTO_SERIES` series (a movie or a loose episode counts for none).
fn pick(rows: Vec<PlayedRow>, caps: Caps) -> Vec<PlayedRow> {
    if caps == Caps::Unlimited {
        return rows;
    }
    let mut series: HashSet<String> = HashSet::new();
    let mut picked = Vec::new();
    for row in rows {
        if picked.len() >= AUTO_ROWS {
            break;
        }
        if let Some(id) = &row.series_id
            && !series.contains(id)
        {
            if series.len() >= AUTO_SERIES {
                continue;
            }
            series.insert(id.clone());
        }
        picked.push(row);
    }
    picked
}

/// Rows Trakt has that the last full read did not list, sorted out.
#[derive(Debug, Default)]
struct Unseen<'a> {
    /// Still on the server and no longer played: Trakt's play goes.
    remove: Vec<&'a BaseItem>,
    /// Gone from the server, or played after all: only the mirror row goes.
    forget: Vec<String>,
}

/// Sorts the unseen ids by what the server says about them. An item the
/// server no longer has is never removed from Trakt.
fn split_unseen<'a>(ids: &[String], found: &'a [BaseItem]) -> Unseen<'a> {
    let by_id: HashMap<&str, &BaseItem> = found.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut split = Unseen::default();
    for id in ids {
        match by_id.get(id.as_str()) {
            Some(item) if !is_played(item) => split.remove.push(item),
            _ => split.forget.push(id.clone()),
        }
    }
    split
}

fn is_played(item: &BaseItem) -> bool {
    item.user_data.as_ref().is_some_and(|data| data.played)
}

/// Runs `work` on the mirror for the session's server and user, off the
/// async threads.
async fn with_db<T, F>(session: &SessionRef, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Db, LibraryScope<'_>) -> Result<T> + Send + 'static,
{
    let Some(server_id) = session.server_id.clone() else {
        bail!("the session has no server id");
    };
    let user_id = session.user_id.clone();
    tokio::task::spawn_blocking(move || {
        let db = Db::open_default()?;
        work(
            &db,
            LibraryScope {
                server_id: &server_id,
                user_id: &user_id,
            },
        )
    })
    .await
    .context("library task failed")?
}

/// Why a failed Emby call ends the pass.
fn emby_stop(error: &anyhow::Error) -> String {
    if is_network(error) {
        "no connection to Emby".to_string()
    } else {
        format!("Emby: {error:#}")
    }
}

impl Tracker {
    /// Reads the played list, then sends to Trakt what it lacks. The caller
    /// holds `flushing` and has checked that Trakt is active.
    pub(super) async fn library_pass(
        &self,
        session: &SessionRef,
        scan: Scan,
        caps: Caps,
    ) -> LibraryReport {
        let mut report = LibraryReport::default();
        if let Err(e) = self
            .run_library_pass(session, scan, caps, &mut report)
            .await
        {
            report.stopped = Some(format!("{e:#}"));
        }
        report.left = with_db(session, |db, scope| db.library_counts(scope))
            .await
            .map(|counts| counts.pending)
            .unwrap_or(0);
        self.set_library_progress("");
        tracing::info!("Trakt library sync: {}", report.summary());
        report
    }

    async fn run_library_pass(
        &self,
        session: &SessionRef,
        scan: Scan,
        caps: Caps,
        report: &mut LibraryReport,
    ) -> Result<()> {
        self.read_played(session, scan, report).await?;
        // A stopped read still lets the push run for what is already known.
        let pending = with_db(session, |db, scope| db.library_pending(scope)).await?;
        let picked = pick(pending, caps);
        if picked.is_empty() {
            return Ok(());
        }
        let Some(entries) = self.build_entries(session, &picked, report).await? else {
            return Ok(());
        };
        self.send_missing(session, entries, report).await
    }

    /// Step 1: reads the played pages into the mirror; a full read also
    /// settles what is no longer played.
    async fn read_played(
        &self,
        session: &SessionRef,
        scan: Scan,
        report: &mut LibraryReport,
    ) -> Result<()> {
        let stamp = tokens::now();
        let mut scan = scan;
        if scan == Scan::Incremental {
            let last = with_db(session, |db, scope| db.library_full_scan_at(scope)).await?;
            scan = effective_scan(scan, last, stamp);
        }
        let mut start = 0;
        let mut complete = false;
        loop {
            self.set_library_progress(format!("Reading the Emby library… {}", report.listed));
            let page = match session
                .client
                .played_page(&session.user_id, start, PAGE)
                .await
            {
                Ok(page) => page,
                Err(e) => {
                    report.stopped = Some(emby_stop(&e));
                    break;
                }
            };
            let read = page.len();
            let rows: Vec<PlayedRow> = page.iter().map(played_row).collect();
            let new = with_db(session, move |db, scope| {
                db.record_played(scope, &rows, stamp)
            })
            .await?;
            report.listed += read;
            report.new += new;
            start += read;
            if read < PAGE {
                complete = true;
                break;
            }
            // Newest plays come first: a page of known rows ends an incremental read.
            if scan != Scan::Full && new == 0 {
                break;
            }
        }
        if scan == Scan::Full && complete && self.settle_unseen(session, stamp, report).await? {
            with_db(session, move |db, scope| {
                db.retry_library_no_ids(scope)?;
                db.prune_library(scope, stamp)?;
                db.set_library_full_scan(scope, stamp)
            })
            .await?;
        }
        Ok(())
    }

    /// After a full read: what Trakt has but the server no longer lists as
    /// played. With "sync unwatched" on, an item that is still on the server
    /// and unplayed loses its Trakt play; otherwise (and for items the server
    /// no longer has) only the mirror forgets it. Incremental reads can't see
    /// unplays, so only the weekly, first and manual full reads get here.
    /// `false`: stopped before everything was settled, so nothing is pruned.
    async fn settle_unseen(
        &self,
        session: &SessionRef,
        stamp: i64,
        report: &mut LibraryReport,
    ) -> Result<bool> {
        let remove_plays = Settings::load().unwrap_or_default().tracking.sync_unwatched;
        if !remove_plays {
            return Ok(true);
        }
        let (unseen, on_trakt) = with_db(session, move |db, scope| {
            Ok((
                db.library_unseen_on_trakt(scope, stamp)?,
                db.library_counts(scope)?.on_trakt,
            ))
        })
        .await?;
        if too_many_unplays(unseen.len(), on_trakt) {
            tracing::warn!(
                "Trakt library sync: {} items are no longer played on the server; too many to remove from Trakt automatically (library rebuild?), skipped",
                unseen.len()
            );
            report.stopped = Some(format!(
                "{} items are no longer played on the server, too many to remove from Trakt automatically",
                unseen.len()
            ));
            return Ok(false);
        }
        let mut shows: HashMap<String, ShowIds> = HashMap::new();
        let mut last: Option<Instant> = None;
        for ids in unseen.chunks(ITEM_CHUNK) {
            let found = match session.client.items_by_ids(&session.user_id, ids).await {
                Ok(found) => found,
                Err(e) => {
                    report.stopped = Some(emby_stop(&e));
                    return Ok(false);
                }
            };
            let split = split_unseen(ids, &found);
            let mut entries: Vec<(String, HistoryEntry)> = Vec::new();
            let mut forget = split.forget;
            for item in split.remove {
                match self.history_entry(session, item, None, &mut shows).await {
                    Ok(Some(entry)) => entries.push((item.id.clone(), entry)),
                    Ok(None) => forget.push(item.id.clone()),
                    Err(e) if is_network(&e) => {
                        report.stopped = Some(emby_stop(&e));
                        return Ok(false);
                    }
                    Err(e) => tracing::warn!("Trakt library sync: item {} skipped: {e:#}", item.id),
                }
            }
            for batch in entries.chunks(SEND_CHUNK) {
                if let Some(sent) = last {
                    tokio::time::sleep(TRAKT_GAP.saturating_sub(sent.elapsed())).await;
                }
                let refs: Vec<&HistoryEntry> = batch.iter().map(|(_, entry)| entry).collect();
                let result = self.trakt.post_history(Direction::Unwatched, &refs).await;
                last = Some(Instant::now());
                match result {
                    Ok(_) => {
                        self.trakt.forget_watched();
                        report.removed += batch.len();
                        forget.extend(batch.iter().map(|(id, _)| id.clone()));
                    }
                    Err(e) => {
                        report.stopped = Some(self.trakt_stop(&e));
                        break;
                    }
                }
            }
            let stopped = report.stopped.is_some();
            with_db(session, move |db, scope| {
                forget
                    .iter()
                    .try_for_each(|id| db.forget_library_item(scope, id))
            })
            .await?;
            if stopped {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The Trakt history entry for an item; `None` when Trakt can't take it
    /// (no usable ids, or an episode without a series).
    async fn history_entry(
        &self,
        session: &SessionRef,
        item: &BaseItem,
        watched_at: Option<i64>,
        shows: &mut HashMap<String, ShowIds>,
    ) -> Result<Option<HistoryEntry>> {
        let Some(trakt_item) = TraktItem::from_item(item) else {
            return Ok(None);
        };
        let episode: Option<ShowEpisode> = show_episode(session, item, shows).await?;
        if trakt_item.kind == TraktKind::Episode && episode.is_none() {
            return Ok(None);
        }
        Ok(Some(HistoryEntry {
            item: trakt_item,
            watched_at,
            episode,
        }))
    }

    /// Step 3: the Trakt entries for the picked rows. Rows whose item is gone
    /// or no longer played are forgotten, rows Trakt can't match are marked.
    /// `None`: the pass stopped.
    async fn build_entries(
        &self,
        session: &SessionRef,
        picked: &[PlayedRow],
        report: &mut LibraryReport,
    ) -> Result<Option<Vec<(String, HistoryEntry)>>> {
        let mut entries: Vec<(String, HistoryEntry)> = Vec::new();
        let mut no_ids: Vec<String> = Vec::new();
        let mut forget: Vec<String> = Vec::new();
        let mut shows: HashMap<String, ShowIds> = HashMap::new();
        'chunks: for chunk in picked.chunks(ITEM_CHUNK) {
            self.set_library_progress("Checking Trakt…");
            let ids: Vec<String> = chunk.iter().map(|row| row.item_id.clone()).collect();
            let found = match session.client.items_by_ids(&session.user_id, &ids).await {
                Ok(found) => found,
                Err(e) => {
                    report.stopped = Some(emby_stop(&e));
                    break;
                }
            };
            let by_id: HashMap<&str, &BaseItem> =
                found.iter().map(|i| (i.id.as_str(), i)).collect();
            for row in chunk {
                let Some(item) = by_id.get(row.item_id.as_str()).filter(|i| is_played(i)) else {
                    forget.push(row.item_id.clone());
                    continue;
                };
                match self
                    .history_entry(session, item, row.played_at, &mut shows)
                    .await
                {
                    Ok(Some(entry)) => entries.push((row.item_id.clone(), entry)),
                    Ok(None) => no_ids.push(row.item_id.clone()),
                    Err(e) if is_network(&e) => {
                        report.stopped = Some(emby_stop(&e));
                        break 'chunks;
                    }
                    // Left pending: tried again next pass.
                    Err(e) => {
                        tracing::warn!("Trakt library sync: item {} skipped: {e:#}", row.item_id)
                    }
                }
            }
        }
        report.no_ids += no_ids.len();
        let now = tokens::now();
        with_db(session, move |db, scope| {
            forget
                .iter()
                .try_for_each(|id| db.forget_library_item(scope, id))?;
            db.set_library_state(scope, &no_ids, TraktState::NoIds, now)
        })
        .await?;
        Ok(Some(entries))
    }

    /// Steps 4 and 5: marks what Trakt already has, sends the rest in
    /// chunks and records each chunk that went through.
    async fn send_missing(
        &self,
        session: &SessionRef,
        entries: Vec<(String, HistoryEntry)>,
        report: &mut LibraryReport,
    ) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let only: Vec<HistoryEntry> = entries.iter().map(|(_, e)| e.clone()).collect();
        let missing = match self.trakt.missing_on_trakt(&only).await {
            Ok(missing) => missing,
            Err(e) => {
                report.stopped = Some(self.trakt_stop(&e));
                return Ok(());
            }
        };
        let (to_send, have): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .zip(missing)
            .partition(|(_, missing)| *missing);
        let have: Vec<String> = have.into_iter().map(|((id, _), _)| id).collect();
        report.already += have.len();
        let now = tokens::now();
        with_db(session, move |db, scope| {
            db.set_library_state(scope, &have, TraktState::OnTrakt, now)
        })
        .await?;

        let total = to_send.len();
        let to_send: Vec<(String, HistoryEntry)> =
            to_send.into_iter().map(|(entry, _)| entry).collect();
        let mut done = 0;
        let mut last: Option<Instant> = None;
        for batch in to_send.chunks(SEND_CHUNK) {
            self.set_library_progress(format!("Sending {done}/{total}…"));
            if let Some(sent) = last {
                tokio::time::sleep(TRAKT_GAP.saturating_sub(sent.elapsed())).await;
            }
            let entries: Vec<HistoryEntry> = batch.iter().map(|(_, e)| e.clone()).collect();
            let result = self.trakt.add_history(&entries).await;
            last = Some(Instant::now());
            match result {
                Ok(outcome) => {
                    let ids: Vec<String> = batch.iter().map(|(id, _)| id.clone()).collect();
                    let skipped = matches!(outcome, Outcome::Skipped(_));
                    let (state, count) = if skipped {
                        (TraktState::NoIds, 0)
                    } else {
                        (TraktState::OnTrakt, ids.len())
                    };
                    if skipped {
                        report.no_ids += ids.len();
                    }
                    let at = tokens::now();
                    with_db(session, move |db, scope| {
                        db.set_library_state(scope, &ids, state, at)
                    })
                    .await?;
                    report.sent += count;
                    done += batch.len();
                }
                Err(e) => {
                    report.stopped = Some(self.trakt_stop(&e));
                    break;
                }
            }
        }
        if report.sent > 0 {
            self.trakt.forget_watched();
        }
        Ok(())
    }

    /// Reports a Trakt failure (toast once, log) and words why the pass ends.
    fn trakt_stop(&self, error: &anyhow::Error) -> String {
        match self.report(ServiceId::Trakt, error) {
            Failure::LoggedOut => "logged out of Trakt".to_string(),
            Failure::Network => "no connection to Trakt".to_string(),
            Failure::Other => format!("Trakt: {error:#}"),
        }
    }

    /// The manual run: reads the whole played list and sends everything
    /// that is pending, whatever the interval. Marks the one-time first sync
    /// done when it ran to the end (rows left pending by a per-item error are
    /// retried by the automatic passes).
    ///
    /// # Errors
    /// No Emby session or server id, or Trakt is not logged in and enabled.
    pub async fn sync_library_now(&'static self) -> Result<LibraryReport> {
        let Some(session) = self.session() else {
            bail!("no Emby session");
        };
        if session.server_id.is_none() {
            bail!("the session has no server id");
        }
        if !self.active().contains(&ServiceId::Trakt) {
            bail!("Trakt is not logged in or not enabled");
        }
        let _guard = self.flushing.lock().await;
        let report = self
            .library_pass(&session, Scan::Full, Caps::Unlimited)
            .await;
        if report.stopped.is_none() {
            let at = tokens::now();
            if let Err(e) = with_db(&session, move |db, scope| {
                db.set_library_first_sync(scope, at)
            })
            .await
            {
                tracing::warn!("library first sync not recorded: {e:#}");
            }
        }
        if report.sent > 0 || report.removed > 0 {
            self.invalidate_lists();
        }
        Ok(report)
    }

    /// Whether the one-time full library sync has finished for the current
    /// server and user (false without a session; a database error counts as
    /// not done).
    pub fn library_first_sync_done(&self) -> bool {
        let Some(session) = self.session() else {
            return false;
        };
        let Some(server_id) = session.server_id.as_deref() else {
            return false;
        };
        let scope = LibraryScope {
            server_id,
            user_id: &session.user_id,
        };
        match Db::open_default().and_then(|db| db.library_first_sync_at(scope)) {
            Ok(at) => at.is_some(),
            Err(e) => {
                tracing::warn!("library first sync unreadable: {e:#}");
                false
            }
        }
    }

    /// What a running library pass is doing ("" when idle).
    pub fn library_progress(&self) -> String {
        self.library_progress
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default()
    }

    fn set_library_progress(&self, text: impl Into<String>) {
        if let Ok(mut slot) = self.library_progress.lock() {
            *slot = text.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emby::models::QueryResult;

    fn page() -> Vec<BaseItem> {
        let json = include_str!("../testdata/emby-played-page.json");
        serde_json::from_str::<QueryResult<BaseItem>>(json)
            .expect("the fixture is a valid Items answer")
            .items
    }

    fn row(id: &str, series: Option<&str>) -> PlayedRow {
        PlayedRow {
            item_id: id.to_string(),
            series_id: series.map(str::to_string),
            played_at: None,
        }
    }

    #[test]
    fn played_rows_carry_series_and_play_date() {
        let items = page();
        let rows: Vec<PlayedRow> = items.iter().map(played_row).collect();
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0].item_id, "m1");
        assert_eq!(rows[0].series_id, None);
        assert_eq!(rows[0].played_at, Some(1_700_000_000));
        // Episodes keep their series; one has no play date.
        assert_eq!(rows[2].series_id.as_deref(), Some("s1"));
        assert_eq!(rows[2].played_at, Some(1_700_013_600));
        assert_eq!(rows[4].item_id, "e3");
        assert_eq!(rows[4].played_at, None);
        // The movie without ids is still a row; Trakt can't take it later.
        assert!(super::TraktItem::from_item(&items[1]).is_none());
        assert!(super::TraktItem::from_item(&items[0]).is_some());
    }

    #[test]
    fn an_automatic_pass_is_capped_by_rows() {
        let rows: Vec<PlayedRow> = (0..301).map(|n| row(&format!("m{n}"), None)).collect();
        assert_eq!(pick(rows.clone(), Caps::Automatic).len(), AUTO_ROWS);
        assert_eq!(pick(rows, Caps::Unlimited).len(), 301);
    }

    #[test]
    fn an_automatic_pass_is_capped_by_series() {
        let mut rows = Vec::new();
        for series in 0..21 {
            for episode in 0..3 {
                rows.push(row(
                    &format!("s{series}e{episode}"),
                    Some(&format!("series{series:02}")),
                ));
            }
        }
        let picked = pick(rows.clone(), Caps::Automatic);
        let distinct: HashSet<_> = picked.iter().filter_map(|r| r.series_id.clone()).collect();
        assert_eq!(distinct.len(), AUTO_SERIES);
        assert_eq!(picked.len(), AUTO_SERIES * 3);
        assert_eq!(pick(rows, Caps::Unlimited).len(), 63);
    }

    #[test]
    fn movies_do_not_count_as_series() {
        let mut rows: Vec<PlayedRow> = (0..30).map(|n| row(&format!("m{n}"), None)).collect();
        rows.extend((0..20).map(|n| row(&format!("e{n}"), Some(&format!("s{n}")))));
        assert_eq!(pick(rows, Caps::Automatic).len(), 50);
    }

    #[test]
    fn unseen_items_are_removed_only_while_they_exist_unplayed() {
        let mut unplayed = BaseItem {
            id: "unplayed".to_string(),
            ..BaseItem::default()
        };
        unplayed.user_data = Some(crate::emby::models::UserItemData {
            played: false,
            ..Default::default()
        });
        let mut played = unplayed.clone();
        played.id = "played".to_string();
        played.user_data = Some(crate::emby::models::UserItemData {
            played: true,
            ..Default::default()
        });
        let found = [unplayed, played];
        let ids: Vec<String> = ["unplayed", "played", "gone"]
            .iter()
            .map(|id| id.to_string())
            .collect();
        let split = split_unseen(&ids, &found);
        let remove: Vec<&str> = split.remove.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(remove, ["unplayed"]);
        assert_eq!(split.forget, ["played", "gone"]);
    }

    #[test]
    fn only_an_incremental_read_is_upgraded_to_a_full_one() {
        let now = 10 * FULL_SCAN_SECS;
        assert_eq!(effective_scan(Scan::Incremental, None, now), Scan::Full);
        assert_eq!(
            effective_scan(Scan::Incremental, Some(now - FULL_SCAN_SECS - 1), now),
            Scan::Full
        );
        assert_eq!(
            effective_scan(Scan::Incremental, Some(now - 60), now),
            Scan::Incremental
        );
        assert_eq!(effective_scan(Scan::Quick, None, now), Scan::Quick);
        assert_eq!(effective_scan(Scan::Full, Some(now), now), Scan::Full);
    }

    #[test]
    fn a_mass_unplay_is_not_removed_automatically() {
        assert!(!too_many_unplays(0, 0));
        assert!(!too_many_unplays(25, 100));
        assert!(too_many_unplays(26, 100));
        // A big library allows a tenth.
        assert!(!too_many_unplays(250, 2500));
        assert!(too_many_unplays(251, 2500));
    }

    #[test]
    fn the_summary_names_what_happened() {
        let done = LibraryReport {
            listed: 10,
            new: 4,
            sent: 3,
            already: 1,
            ..Default::default()
        };
        assert_eq!(
            done.summary(),
            "10 played items read (4 new), 3 added to Trakt, 1 already there"
        );
        let partial = LibraryReport {
            no_ids: 2,
            removed: 1,
            left: 5,
            stopped: Some("no connection to Trakt".to_string()),
            ..done
        };
        assert_eq!(
            partial.summary(),
            "10 played items read (4 new), 3 added to Trakt, 1 already there, 2 without ids, 1 removed from Trakt, 5 left for later. Stopped: no connection to Trakt"
        );
    }
}
