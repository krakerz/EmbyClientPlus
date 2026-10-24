//! Trakt: device code flow (client id only, no secret), scrobbles by the
//! item's own ids. Refresh tokens are single-use: the new pair is saved
//! before anything else, and refreshes are serialized.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use super::resolve::ShowIds;
use super::tokens::{self, Tokens};
use super::{
    Account, BoxFuture, Direction, HistoryEntry, LoggedOut, LoginInput, Outcome, ScrobbleAction,
    ServiceId, TrackingService, TraktItem, TraktKind, Update, http, is_network,
};

pub const API_URL: &str = "https://api.trakt.tv";
pub const DEVICE_CODE_URL: &str = "https://api.trakt.tv/oauth/device/code";
pub const DEVICE_TOKEN_URL: &str = "https://api.trakt.tv/oauth/device/token";
pub const TOKEN_URL: &str = "https://api.trakt.tv/oauth/token";
pub const CLIENT_ID: &str = "BH2BcEg_aYlDFV9pM82N3nU3Ew6jke40NytzK1EXiAo";

const ID: ServiceId = ServiceId::Trakt;
const REFRESH_MARGIN: i64 = 86_400;

#[derive(Debug, Clone)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
    /// Set when the user closes the dialog; the poll stops.
    pub cancel: Arc<AtomicBool>,
}

#[derive(Deserialize)]
struct RawDeviceCode {
    device_code: String,
    user_code: String,
    verification_url: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

/// Every Trakt call carries these headers (User-Agent comes from `http()`).
fn headers(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header("trakt-api-version", "2")
        .header("trakt-api-key", CLIENT_ID)
        .header("Content-Type", "application/json")
}

fn post(url: &str) -> reqwest::RequestBuilder {
    headers(http().post(url))
}

async fn check(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    bail!(
        "{} {status}: {}",
        ID.label(),
        body.chars().take(200).collect::<String>()
    )
}

/// Asks Trakt for a code to show the user.
pub async fn request_code() -> Result<DeviceCode> {
    let resp = post(DEVICE_CODE_URL)
        .json(&json!({ "client_id": CLIENT_ID }))
        .send()
        .await?;
    let raw: RawDeviceCode = check(resp).await?.json().await?;
    Ok(DeviceCode {
        device_code: raw.device_code,
        user_code: raw.user_code,
        verification_url: raw.verification_url,
        expires_in: raw.expires_in,
        interval: raw.interval.max(1),
        cancel: Arc::new(AtomicBool::new(false)),
    })
}

/// `{"tvdb":…, "tmdb":…, "imdb":"tt…"}` with only the ids present.
fn ids_json(ids: &ShowIds) -> Value {
    let mut map = serde_json::Map::new();
    if let Some(tvdb) = ids.tvdb {
        map.insert("tvdb".into(), json!(tvdb));
    }
    if let Some(tmdb) = ids.tmdb {
        map.insert("tmdb".into(), json!(tmdb));
    }
    if let Some(imdb) = &ids.imdb {
        map.insert("imdb".into(), json!(imdb));
    }
    Value::Object(map)
}

fn single_key(kind: TraktKind) -> &'static str {
    match kind {
        TraktKind::Episode => "episode",
        TraktKind::Movie => "movie",
    }
}

fn plural_key(kind: TraktKind) -> &'static str {
    match kind {
        TraktKind::Episode => "episodes",
        TraktKind::Movie => "movies",
    }
}

fn action_path(action: ScrobbleAction) -> &'static str {
    match action {
        ScrobbleAction::Start => "start",
        ScrobbleAction::Pause => "pause",
        ScrobbleAction::Stop => "stop",
    }
}

/// Scrobbles Trakt answers with a 422, so they aren't sent: a pause or stop
/// below 1 %, and a pause from 80 % on (the later stop scrobbles it).
fn scrobble_refused(action: ScrobbleAction, progress: f32) -> bool {
    match action {
        ScrobbleAction::Start => false,
        ScrobbleAction::Pause => !(1.0..80.0).contains(&progress),
        ScrobbleAction::Stop => progress < 1.0,
    }
}

fn scrobble_body(item: &TraktItem, progress: f32) -> Value {
    json!({ single_key(item.kind): { "ids": ids_json(&item.ids) }, "progress": progress })
}

/// One `/sync/history` (or `/sync/history/remove`) body for all entries:
/// `{"episodes":[…],"movies":[…]}`, empty lists left out. Only the add form
/// carries `watched_at`.
fn history_batch_body(entries: &[&HistoryEntry], direction: Direction) -> Value {
    let mut body = serde_json::Map::new();
    for kind in [TraktKind::Episode, TraktKind::Movie] {
        let list: Vec<Value> = entries
            .iter()
            .filter(|entry| entry.item.kind == kind)
            .map(|entry| {
                let mut value = json!({ "ids": ids_json(&entry.item.ids) });
                if direction == Direction::Watched {
                    value["watched_at"] = json!(
                        entry
                            .watched_at
                            .map_or_else(|| "released".to_string(), rfc3339)
                    );
                }
                value
            })
            .collect();
        if !list.is_empty() {
            body.insert(plural_key(kind).to_string(), Value::Array(list));
        }
    }
    Value::Object(body)
}

/// How many episodes/movies a sync answer lists under `not_found`.
fn not_found_count(body: &Value) -> usize {
    ["episodes", "movies"]
        .into_iter()
        .filter_map(|key| body.pointer(&format!("/not_found/{key}")))
        .filter_map(Value::as_array)
        .map(Vec::len)
        .sum()
}

/// The episodes Trakt lists as watched for one show: (season, episode).
pub type Watched = HashSet<(u32, u32)>;

/// Names a show by its first known id (tvdb, tmdb, imdb); `None` without any.
fn show_key(ids: &ShowIds) -> Option<String> {
    ids.tvdb
        .map(|id| format!("tvdb:{id}"))
        .or_else(|| ids.tmdb.map(|id| format!("tmdb:{id}")))
        .or_else(|| ids.imdb.as_ref().map(|id| format!("imdb:{id}")))
}

/// The `/search/<id type>/<id>?type=show` lookups to try, in order.
fn show_lookups(ids: &ShowIds) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(id) = ids.tvdb {
        paths.push(format!("search/tvdb/{id}?type=show"));
    }
    if let Some(id) = ids.tmdb {
        paths.push(format!("search/tmdb/{id}?type=show"));
    }
    if let Some(id) = &ids.imdb {
        paths.push(format!("search/imdb/{id}?type=show"));
    }
    paths
}

/// The show's Trakt id in a search answer.
fn search_trakt_id(body: &Value) -> Option<u64> {
    body.as_array()?
        .iter()
        .find_map(|hit| hit.pointer("/show/ids/trakt").and_then(Value::as_u64))
}

/// The completed episodes in a `/shows/<id>/progress/watched` answer.
pub(super) fn watched_from_progress(body: &Value) -> Watched {
    let number = |value: &Value| {
        value
            .get("number")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    };
    let mut watched = Watched::new();
    for season in body
        .get("seasons")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(season_number) = number(season) else {
            continue;
        };
        for episode in season
            .get("episodes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if episode.get("completed").and_then(Value::as_bool) == Some(true)
                && let Some(episode_number) = number(episode)
            {
                watched.insert((season_number, episode_number));
            }
        }
    }
    watched
}

/// Trakt's (season, episode) for each episode id. Trakt numbers like TMDB,
/// Emby like TVDB (Oshi no Ko: TVDB S1–S3 is one long Trakt season), so an
/// Emby episode is translated through its own ids, never by its numbers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EpisodeMap {
    tvdb: HashMap<u32, (u32, u32)>,
    tmdb: HashMap<u32, (u32, u32)>,
    imdb: HashMap<String, (u32, u32)>,
}

impl EpisodeMap {
    /// The episode with these ids: tvdb first, then tmdb, then imdb.
    fn find(&self, ids: &ShowIds) -> Option<(u32, u32)> {
        ids.tvdb
            .and_then(|id| self.tvdb.get(&id))
            .or_else(|| ids.tmdb.and_then(|id| self.tmdb.get(&id)))
            .or_else(|| ids.imdb.as_ref().and_then(|id| self.imdb.get(id)))
            .copied()
    }
}

/// The ids of every episode in a `/shows/<id>/seasons?extended=episodes` answer.
pub(super) fn episode_map_from_seasons(body: &Value) -> EpisodeMap {
    let mut map = EpisodeMap::default();
    for season in body.as_array().into_iter().flatten() {
        for episode in season
            .get("episodes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let number = |key: &str| {
                episode
                    .get(key)
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            };
            let (Some(s), Some(n)) = (number("season"), number("number")) else {
                continue;
            };
            let id = |key: &str| {
                episode
                    .pointer(&format!("/ids/{key}"))
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
            };
            if let Some(tvdb) = id("tvdb") {
                map.tvdb.insert(tvdb, (s, n));
            }
            if let Some(tmdb) = id("tmdb") {
                map.tmdb.insert(tmdb, (s, n));
            }
            if let Some(imdb) = episode.pointer("/ids/imdb").and_then(Value::as_str) {
                map.imdb.insert(imdb.to_string(), (s, n));
            }
        }
    }
    map
}

/// The movies Trakt lists as watched, by TMDB and IMDb id.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct WatchedMovies {
    tmdb: HashSet<u32>,
    imdb: HashSet<String>,
}

impl WatchedMovies {
    /// Whether Trakt lists the movie (tmdb first, then imdb); `None` when
    /// the ids hold neither, so there is nothing to match by.
    pub fn watched(&self, ids: &ShowIds) -> Option<bool> {
        if ids.tmdb.is_none() && ids.imdb.is_none() {
            return None;
        }
        let by_tmdb = ids.tmdb.is_some_and(|id| self.tmdb.contains(&id));
        let by_imdb = ids.imdb.as_ref().is_some_and(|id| self.imdb.contains(id));
        Some(by_tmdb || by_imdb)
    }
}

/// The movies in a `/sync/watched/movies` answer.
pub(super) fn movies_from_watched(body: &Value) -> WatchedMovies {
    let mut movies = WatchedMovies::default();
    for entry in body.as_array().into_iter().flatten() {
        if let Some(id) = entry
            .pointer("/movie/ids/tmdb")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
        {
            movies.tmdb.insert(id);
        }
        if let Some(id) = entry.pointer("/movie/ids/imdb").and_then(Value::as_str) {
            movies.imdb.insert(id.to_string());
        }
    }
    movies
}

/// What Trakt has on one show: the watched episodes and the numbering.
#[derive(Clone)]
pub struct ShowState {
    pub watched: Arc<Watched>,
    pub episodes: Arc<EpisodeMap>,
}

impl ShowState {
    /// Whether Trakt lists the episode (and the `count - 1` after it, for a
    /// file covering several) as watched; `None` when its ids don't lead to
    /// a Trakt episode.
    pub fn episode_watched(&self, ids: &ShowIds, count: u32) -> Option<bool> {
        let (season, number) = self.episodes.find(ids)?;
        Some((0..count.max(1)).all(|extra| self.watched.contains(&(season, number + extra))))
    }
}

/// The entries to add: not those Trakt already lists as watched. A movie, a
/// show missing from `progress` (unknown to Trakt, or the read failed) and an
/// episode that can't be translated keep their entries: a rare duplicate
/// beats a lost play.
fn leave_out_watched<'a>(
    entries: &[&'a HistoryEntry],
    progress: &HashMap<String, ShowState>,
) -> Vec<&'a HistoryEntry> {
    entries
        .iter()
        .copied()
        .filter(|entry| {
            let Some(episode) = &entry.episode else {
                return true;
            };
            let state = show_key(&episode.show).and_then(|key| progress.get(&key));
            let seen = state.and_then(|state| state.episode_watched(&episode.ids, episode.count));
            seen != Some(true)
        })
        .collect()
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
fn rfc3339(secs: i64) -> String {
    let (year, month, day) = crate::logging::civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

pub struct Trakt {
    tokens: Mutex<Option<Tokens>>,
    refresh_lock: tokio::sync::Mutex<()>,
    /// Show key → Trakt id (`None`: Trakt doesn't know the show); per session.
    show_ids: Mutex<HashMap<String, Option<u64>>>,
    /// Trakt show id → its watched episodes; dropped after our own history writes.
    progress: Mutex<HashMap<u64, Arc<Watched>>>,
    /// Trakt show id → its episode numbering (stable, kept for the session).
    numbering: Mutex<HashMap<u64, Arc<EpisodeMap>>>,
    /// The watched movies, read once per session; dropped after our own writes.
    movies: Mutex<Option<Arc<WatchedMovies>>>,
}

impl Default for Trakt {
    fn default() -> Self {
        Self::new()
    }
}

impl Trakt {
    pub fn new() -> Self {
        Trakt {
            tokens: Mutex::new(tokens::load(ID)),
            refresh_lock: tokio::sync::Mutex::new(()),
            show_ids: Mutex::new(HashMap::new()),
            progress: Mutex::new(HashMap::new()),
            numbering: Mutex::new(HashMap::new()),
            movies: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Tokens>> {
        self.tokens.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn snapshot(&self) -> Option<Tokens> {
        self.lock().clone()
    }

    fn access(&self) -> Result<String> {
        self.lock()
            .as_ref()
            .map(|t| t.access.clone())
            .ok_or_else(|| LoggedOut(ID).into())
    }

    fn drop_tokens(&self) {
        if let Err(e) = tokens::delete(ID) {
            tracing::warn!("{} tokens not deleted: {e:#}", ID.label());
        }
        *self.lock() = None;
    }

    /// Single-use refresh tokens: serialized, and the new pair is saved first.
    async fn renew(&self, stale: Option<&str>) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        let Some(current) = self.snapshot() else {
            return Err(LoggedOut(ID).into());
        };
        match stale {
            Some(stale) if current.access != stale => return Ok(()),
            None if !current.expires_within(REFRESH_MARGIN) => return Ok(()),
            _ => {}
        }
        let Some(refresh) = current.refresh.clone() else {
            self.drop_tokens();
            return Err(LoggedOut(ID).into());
        };
        let resp = post(TOKEN_URL)
            .json(&json!({
                "refresh_token": refresh,
                "client_id": CLIENT_ID,
                "redirect_uri": "urn:ietf:wg:oauth:2.0:oob",
                "grant_type": "refresh_token",
            }))
            .send()
            .await?;
        if matches!(resp.status().as_u16(), 400 | 401) {
            self.drop_tokens();
            return Err(LoggedOut(ID).into());
        }
        let body: TokenResponse = check(resp).await?.json().await?;
        let renewed = Tokens {
            access: body.access_token,
            refresh: body.refresh_token.or(current.refresh),
            expires_at: tokens::expires_at(body.expires_in),
            username: current.username,
            user_id: current.user_id,
        };
        tokens::save(ID, &renewed)?;
        *self.lock() = Some(renewed);
        Ok(())
    }

    /// Sends with the bearer token (refreshing first, and once on a 401).
    /// The status is the caller's to judge, except 401.
    async fn send(&self, build: impl Fn() -> reqwest::RequestBuilder) -> Result<reqwest::Response> {
        self.renew(None).await?;
        let access = self.access()?;
        let mut resp = headers(build()).bearer_auth(&access).send().await?;
        if resp.status().as_u16() == 401 {
            self.renew(Some(&access)).await?;
            let access = self.access()?;
            resp = headers(build()).bearer_auth(&access).send().await?;
            if resp.status().as_u16() == 401 {
                self.drop_tokens();
                return Err(LoggedOut(ID).into());
            }
        }
        Ok(resp)
    }

    async fn login_device(&self, code: DeviceCode) -> Result<Account> {
        let expired = || anyhow::anyhow!("The code expired, try again");
        let deadline = Instant::now() + Duration::from_secs(code.expires_in);
        let mut interval = code.interval.max(1);
        let body: TokenResponse = loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            if code.cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            if Instant::now() >= deadline {
                return Err(expired());
            }
            let resp = match post(DEVICE_TOKEN_URL)
                .json(&json!({ "code": code.device_code, "client_id": CLIENT_ID }))
                .send()
                .await
            {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::debug!("Trakt device poll: {e}");
                    continue;
                }
            };
            match resp.status().as_u16() {
                200 => break resp.json().await?,
                400 => {}
                429 => interval += 5,
                404 => bail!("Invalid code"),
                409 => bail!("Code already used"),
                410 => return Err(expired()),
                418 => bail!("Denied on trakt.tv"),
                other => bail!("Trakt {other} while waiting for the code"),
            }
        };
        let settings: Value = check(
            headers(http().get(format!("{API_URL}/users/settings")))
                .bearer_auth(&body.access_token)
                .send()
                .await?,
        )
        .await?
        .json()
        .await?;
        let name = settings
            .pointer("/user/username")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Trakt: no username in the answer"))?
            .to_string();
        let new = Tokens {
            access: body.access_token,
            refresh: body.refresh_token,
            expires_at: tokens::expires_at(body.expires_in),
            username: name.clone(),
            user_id: None,
        };
        tokens::save(ID, &new)?;
        *self.lock() = Some(new);
        Ok(Account { name })
    }

    async fn scrobble(
        &self,
        item: &TraktItem,
        action: ScrobbleAction,
        progress: f32,
    ) -> Result<Outcome> {
        if scrobble_refused(action, progress) {
            return Ok(Outcome::Unchanged);
        }
        let url = format!("{API_URL}/scrobble/{}", action_path(action));
        let body = scrobble_body(item, progress);
        let resp = self.send(|| http().post(&url).json(&body)).await?;
        match resp.status().as_u16() {
            409 => Ok(Outcome::Unchanged),
            404 => Ok(Outcome::Skipped(
                "Trakt doesn't know this title".to_string(),
            )),
            _ => {
                check(resp).await?;
                if action == ScrobbleAction::Stop {
                    // A finished play changes the watched list.
                    self.forget_watched();
                }
                Ok(Outcome::Done)
            }
        }
    }

    /// Drops the session's copies of the watched lists (shows and movies).
    fn forget_watched(&self) {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        *self.movies.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The user's watched movies: one call per session, until our own writes.
    pub async fn watched_movies(&self) -> Result<Arc<WatchedMovies>> {
        let cached = self
            .movies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(movies) = cached {
            return Ok(movies);
        }
        let body = self
            .get_json("sync/watched/movies")
            .await?
            .unwrap_or(Value::Null);
        let movies = Arc::new(movies_from_watched(&body));
        *self.movies.lock().unwrap_or_else(|e| e.into_inner()) = Some(movies.clone());
        Ok(movies)
    }

    /// GET with the bearer token; `None` on a 404.
    async fn get_json(&self, path: &str) -> Result<Option<Value>> {
        let url = format!("{API_URL}/{path}");
        let resp = self.send(|| http().get(&url)).await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        Ok(Some(check(resp).await?.json().await?))
    }

    /// The show's Trakt id by its tvdb/tmdb/imdb id (cached for the session).
    async fn show_id(&self, ids: &ShowIds) -> Result<Option<u64>> {
        let Some(key) = show_key(ids) else {
            return Ok(None);
        };
        let cached = self
            .show_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .copied();
        if let Some(found) = cached {
            return Ok(found);
        }
        let mut found = None;
        for path in show_lookups(ids) {
            if let Some(body) = self.get_json(&path).await? {
                found = search_trakt_id(&body);
                if found.is_some() {
                    break;
                }
            }
        }
        self.show_ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, found);
        Ok(found)
    }

    /// The show's watched episodes and numbering on Trakt; `None` when Trakt
    /// doesn't know the show. `fresh` reads the watched list again instead
    /// of using the session's copy.
    pub async fn show_state(&self, ids: &ShowIds, fresh: bool) -> Result<Option<ShowState>> {
        let Some(trakt_id) = self.show_id(ids).await? else {
            return Ok(None);
        };
        let cached = self
            .progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&trakt_id)
            .cloned();
        let watched = match cached {
            Some(watched) if !fresh => watched,
            _ => {
                let Some(body) = self
                    .get_json(&format!("shows/{trakt_id}/progress/watched?specials=true"))
                    .await?
                else {
                    return Ok(None);
                };
                let watched = Arc::new(watched_from_progress(&body));
                self.progress
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(trakt_id, watched.clone());
                watched
            }
        };
        let known = self
            .numbering
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&trakt_id)
            .cloned();
        let episodes = match known {
            Some(episodes) => episodes,
            None => {
                let body = self
                    .get_json(&format!("shows/{trakt_id}/seasons?extended=episodes"))
                    .await?
                    .unwrap_or(Value::Null);
                let episodes = Arc::new(episode_map_from_seasons(&body));
                self.numbering
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(trakt_id, episodes.clone());
                episodes
            }
        };
        Ok(Some(ShowState { watched, episodes }))
    }

    /// Reads the progress of every show in the batch and drops the entries
    /// Trakt already has. Only a lost connection or login fails the call; a
    /// show whose read fails otherwise is added unchecked.
    async fn leave_out_known<'a>(
        &self,
        entries: Vec<&'a HistoryEntry>,
    ) -> Result<Vec<&'a HistoryEntry>> {
        let mut progress: HashMap<String, ShowState> = HashMap::new();
        let mut asked: HashSet<String> = HashSet::new();
        for entry in &entries {
            let Some(episode) = &entry.episode else {
                continue;
            };
            let Some(key) = show_key(&episode.show) else {
                continue;
            };
            if !asked.insert(key.clone()) {
                continue;
            }
            match self.show_state(&episode.show, true).await {
                Ok(Some(state)) => {
                    progress.insert(key, state);
                }
                Ok(None) => {}
                Err(e) if is_network(&e) || e.chain().any(|c| c.is::<LoggedOut>()) => {
                    return Err(e);
                }
                Err(e) => tracing::warn!("Trakt progress of {key} unreadable, adding all: {e:#}"),
            }
        }
        Ok(leave_out_watched(&entries, &progress))
    }

    async fn history_batch(
        &self,
        direction: Direction,
        entries: &[HistoryEntry],
    ) -> Result<Outcome> {
        let mut unique: Vec<&HistoryEntry> = Vec::new();
        for entry in entries {
            if !unique.iter().any(|seen| seen.item == entry.item) {
                unique.push(entry);
            }
        }
        let send = match direction {
            Direction::Watched => self.leave_out_known(unique).await?,
            Direction::Unwatched => unique,
        };
        if send.is_empty() {
            return Ok(Outcome::Unchanged);
        }
        let path = match direction {
            Direction::Watched => "sync/history",
            Direction::Unwatched => "sync/history/remove",
        };
        let body = history_batch_body(&send, direction);
        let url = format!("{API_URL}/{path}");
        let answer: Value = check(self.send(|| http().post(&url).json(&body)).await?)
            .await?
            .json()
            .await
            .unwrap_or(Value::Null);
        self.forget_watched();
        if not_found_count(&answer) >= send.len() {
            return Ok(Outcome::Skipped(
                "Trakt doesn't know this title".to_string(),
            ));
        }
        Ok(Outcome::Done)
    }
}

impl TrackingService for Trakt {
    fn id(&self) -> ServiceId {
        ID
    }

    fn is_logged_in(&self) -> bool {
        self.lock().is_some()
    }

    fn account(&self) -> Option<Account> {
        self.lock().as_ref().map(|t| Account {
            name: t.username.clone(),
        })
    }

    fn expires_at(&self) -> Option<i64> {
        self.lock().as_ref().and_then(|t| t.expires_at)
    }

    fn login(&self, input: LoginInput) -> BoxFuture<'_, Result<Account>> {
        Box::pin(async move {
            match input {
                LoginInput::DeviceCode(code) => self.login_device(code).await,
                _ => bail!("Trakt logs in with a device code"),
            }
        })
    }

    fn logout(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            let result = tokens::delete(ID);
            *self.lock() = None;
            result
        })
    }

    fn refresh(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move { self.renew(None).await })
    }

    fn update<'a>(&'a self, update: &'a Update) -> BoxFuture<'a, Result<Outcome>> {
        Box::pin(async move {
            match update {
                Update::Scrobble {
                    item,
                    action,
                    progress,
                } => self.scrobble(item, *action, *progress).await,
                Update::HistoryBatch { direction, entries } => {
                    self.history_batch(*direction, entries).await
                }
                Update::Progress { .. } => Ok(Outcome::Unchanged),
            }
        })
    }

    fn show_progress<'a>(&'a self, ids: &'a ShowIds) -> BoxFuture<'a, Result<Option<ShowState>>> {
        Box::pin(async move { self.show_state(ids, false).await })
    }

    fn watched_movies(&self) -> BoxFuture<'_, Result<Arc<WatchedMovies>>> {
        Box::pin(Trakt::watched_movies(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracking::ShowEpisode;

    fn item(kind: TraktKind) -> TraktItem {
        TraktItem {
            kind,
            ids: ShowIds {
                tvdb: Some(42),
                imdb: Some("tt123".to_string()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn rfc3339_from_unix() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400 + 3661), "2000-02-29T01:01:01Z");
        assert_eq!(rfc3339(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn ids_only_present() {
        assert_eq!(
            ids_json(&item(TraktKind::Episode).ids),
            json!({"tvdb":42,"imdb":"tt123"})
        );
    }

    fn entry(kind: TraktKind, tvdb: u32, show: u32, season: u32, number: u32) -> HistoryEntry {
        // Numbers are unused here: an episode is translated by its ids, and
        // `SEASONS` gives tvdb `n` the Trakt number (1, n).
        let _ = (season, number);
        HistoryEntry {
            item: TraktItem {
                kind,
                ids: ShowIds {
                    tvdb: Some(tvdb),
                    ..Default::default()
                },
            },
            watched_at: Some(0),
            episode: (kind == TraktKind::Episode).then(|| ShowEpisode {
                show: ShowIds {
                    tvdb: Some(show),
                    ..Default::default()
                },
                ids: ShowIds {
                    tvdb: Some(tvdb),
                    ..Default::default()
                },
                count: 1,
            }),
        }
    }

    /// An Oshi-no-Ko-like show: Trakt has one long season 1; Emby (TVDB)
    /// calls tvdb 912 "S2E1" but Trakt knows it as S1E12.
    const SEASONS: &str = r#"[
        {"number":1,"ids":{"trakt":1},"episodes":[
            {"season":1,"number":1,"ids":{"trakt":11,"tvdb":901,"tmdb":501,"imdb":"tt01"}},
            {"season":1,"number":2,"ids":{"trakt":12,"tvdb":902,"tmdb":502,"imdb":null}},
            {"season":1,"number":12,"ids":{"trakt":22,"tvdb":912,"tmdb":512,"imdb":null}},
            {"season":1,"number":13,"ids":{"trakt":23,"tvdb":913,"tmdb":513,"imdb":null}}]}]"#;

    fn state(progress: &str) -> ShowState {
        ShowState {
            watched: Arc::new(watched_from_progress(
                &serde_json::from_str(progress).unwrap(),
            )),
            episodes: Arc::new(episode_map_from_seasons(
                &serde_json::from_str(SEASONS).unwrap(),
            )),
        }
    }

    fn ids(tvdb: Option<u32>, tmdb: Option<u32>, imdb: Option<&str>) -> ShowIds {
        ShowIds {
            tvdb,
            tmdb,
            imdb: imdb.map(str::to_string),
            ..Default::default()
        }
    }

    const PROGRESS: &str = r#"{"aired":4,"completed":2,"seasons":[
        {"number":1,"aired":2,"completed":2,"episodes":[
            {"number":1,"completed":true,"last_watched_at":"2024-01-01T00:00:00.000Z"},
            {"number":2,"completed":true,"last_watched_at":"2024-01-02T00:00:00.000Z"}]},
        {"number":2,"aired":2,"completed":0,"episodes":[
            {"number":1,"completed":false,"last_watched_at":null},
            {"number":2,"completed":false,"last_watched_at":null}]}]}"#;

    #[test]
    fn scrobble_guard_skips_what_trakt_rejects() {
        use ScrobbleAction::{Pause, Start, Stop};
        assert!(scrobble_refused(Pause, 0.5));
        assert!(scrobble_refused(Stop, 0.5));
        assert!(scrobble_refused(Pause, 80.0));
        assert!(scrobble_refused(Pause, 90.7));
        assert!(!scrobble_refused(Pause, 1.0));
        assert!(!scrobble_refused(Pause, 79.9));
        assert!(!scrobble_refused(Stop, 1.0));
        assert!(!scrobble_refused(Stop, 95.0));
        assert!(!scrobble_refused(Start, 0.0));
    }

    #[test]
    fn scrobble_body_shape() {
        assert_eq!(
            scrobble_body(&item(TraktKind::Movie), 95.0),
            json!({"movie":{"ids":{"tvdb":42,"imdb":"tt123"}},"progress":95.0})
        );
    }

    #[test]
    fn history_batch_is_one_body() {
        let a = entry(TraktKind::Episode, 1, 9, 1, 1);
        let b = entry(TraktKind::Episode, 2, 9, 1, 2);
        let m = entry(TraktKind::Movie, 3, 0, 0, 0);
        assert_eq!(
            history_batch_body(&[&a, &b, &m], Direction::Watched),
            json!({
                "episodes":[
                    {"ids":{"tvdb":1},"watched_at":"1970-01-01T00:00:00Z"},
                    {"ids":{"tvdb":2},"watched_at":"1970-01-01T00:00:00Z"}],
                "movies":[{"ids":{"tvdb":3},"watched_at":"1970-01-01T00:00:00Z"}]
            })
        );
        // A backfilled episode without a play date goes in at its release date.
        let released = HistoryEntry {
            watched_at: None,
            ..entry(TraktKind::Episode, 4, 9, 1, 4)
        };
        assert_eq!(
            history_batch_body(&[&released], Direction::Watched),
            json!({"episodes":[{"ids":{"tvdb":4},"watched_at":"released"}]})
        );
        // The remove form has no timestamps and no empty lists.
        assert_eq!(
            history_batch_body(&[&a, &b], Direction::Unwatched),
            json!({"episodes":[{"ids":{"tvdb":1}},{"ids":{"tvdb":2}}]})
        );
    }

    #[test]
    fn not_found_counting() {
        let body = json!({"added":{"episodes":0},"not_found":{"episodes":[{"ids":{}}],"movies":[{"ids":{}}]}});
        assert_eq!(not_found_count(&body), 2);
        assert_eq!(not_found_count(&json!({"added":{}})), 0);
    }

    #[test]
    fn progress_and_search_answers() {
        let watched = watched_from_progress(&serde_json::from_str(PROGRESS).unwrap());
        assert_eq!(watched, Watched::from([(1, 1), (1, 2)]));
        assert!(watched_from_progress(&json!({})).is_empty());
        let found =
            json!([{"type":"show","score":1.0,"show":{"title":"X","ids":{"trakt":1388,"tvdb":9}}}]);
        assert_eq!(search_trakt_id(&found), Some(1388));
        assert_eq!(search_trakt_id(&json!([])), None);
    }

    /// Trakt's watched list: S1E12 (Emby's "S2E1") and S1E1.
    const OSHI_PROGRESS: &str = r#"{"seasons":[{"number":1,"episodes":[
        {"number":1,"completed":true},{"number":2,"completed":false},
        {"number":12,"completed":true},{"number":13,"completed":false}]}]}"#;

    #[test]
    fn episodes_translate_through_their_ids_not_numbers() {
        let state = state(OSHI_PROGRESS);
        // tvdb 912 = Emby S2E1 = Trakt S1E12: watched.
        assert_eq!(
            state.episode_watched(&ids(Some(912), None, None), 1),
            Some(true)
        );
        // tvdb 913 = Trakt S1E13: not watched.
        assert_eq!(
            state.episode_watched(&ids(Some(913), None, None), 1),
            Some(false)
        );
        // Only tmdb or imdb known: still found.
        assert_eq!(
            state.episode_watched(&ids(None, Some(512), None), 1),
            Some(true)
        );
        assert_eq!(
            state.episode_watched(&ids(None, None, Some("tt01")), 1),
            Some(true)
        );
        // A file covering two episodes needs both: 912 + 913.
        assert_eq!(
            state.episode_watched(&ids(Some(912), None, None), 2),
            Some(false)
        );
        // No ids, or ids Trakt doesn't have: can't tell.
        assert_eq!(state.episode_watched(&ShowIds::default(), 1), None);
        assert_eq!(state.episode_watched(&ids(Some(1), None, None), 1), None);
    }

    #[test]
    fn already_watched_episodes_are_left_out() {
        let progress = HashMap::from([("tvdb:9".to_string(), state(OSHI_PROGRESS))]);
        let seen = entry(TraktKind::Episode, 912, 9, 2, 1);
        // Emby S1E1 numbers hit Trakt's watched S1E1 only if the ids said so.
        let unseen_by_number = entry(TraktKind::Episode, 913, 9, 1, 1);
        let other_show = entry(TraktKind::Episode, 3, 77, 1, 1);
        let unknown_episode = entry(TraktKind::Episode, 777, 9, 1, 12);
        let mut no_ids = entry(TraktKind::Episode, 5, 9, 1, 1);
        no_ids.episode.as_mut().unwrap().ids = ShowIds::default();
        let movie = entry(TraktKind::Movie, 4, 0, 0, 0);
        let all = [
            &seen,
            &unseen_by_number,
            &other_show,
            &unknown_episode,
            &no_ids,
            &movie,
        ];
        let kept = leave_out_watched(&all, &progress);
        let kept: Vec<u32> = kept.iter().map(|e| e.item.ids.tvdb.unwrap()).collect();
        // Only `seen` is left out; everything unchecked is added.
        assert_eq!(kept, vec![913, 3, 777, 5, 4]);
    }

    #[test]
    fn a_non_anime_show_with_matching_numbers() {
        let seasons = r#"[{"number":1,"episodes":[
            {"season":1,"number":1,"ids":{"tvdb":10,"tmdb":null,"imdb":"tt10"}},
            {"season":1,"number":2,"ids":{"tvdb":11,"tmdb":null,"imdb":"tt11"}}]}]"#;
        let progress = r#"{"seasons":[{"number":1,"episodes":[{"number":1,"completed":true}]}]}"#;
        let state = ShowState {
            watched: Arc::new(watched_from_progress(
                &serde_json::from_str(progress).unwrap(),
            )),
            episodes: Arc::new(episode_map_from_seasons(
                &serde_json::from_str(seasons).unwrap(),
            )),
        };
        assert_eq!(
            state.episode_watched(&ids(Some(10), None, None), 1),
            Some(true)
        );
        assert_eq!(
            state.episode_watched(&ids(Some(11), None, None), 1),
            Some(false)
        );
    }

    #[test]
    fn watched_movies_match_by_tmdb_then_imdb() {
        let body = json!([
            {"plays": 1, "movie": {"title": "Spider-Man", "ids": {"trakt": 1, "tmdb": 557, "imdb": "tt0145487"}}},
            {"plays": 2, "movie": {"title": "No tmdb", "ids": {"trakt": 2, "tmdb": null, "imdb": "tt0000002"}}},
        ]);
        let movies = movies_from_watched(&body);
        let ids = |tmdb: Option<u32>, imdb: Option<&str>| ShowIds {
            tmdb,
            imdb: imdb.map(str::to_string),
            ..ShowIds::default()
        };
        // tmdb match, even with a wrong imdb
        assert_eq!(movies.watched(&ids(Some(557), Some("tt9"))), Some(true));
        // imdb fallback
        assert_eq!(movies.watched(&ids(None, Some("tt0000002"))), Some(true));
        assert_eq!(movies.watched(&ids(Some(1), Some("tt0000002"))), Some(true));
        // known ids, not watched
        assert_eq!(movies.watched(&ids(Some(1), Some("tt1"))), Some(false));
        // nothing to match by (tvdb alone doesn't count)
        let tvdb_only = ShowIds {
            tvdb: Some(5),
            ..ShowIds::default()
        };
        assert_eq!(movies.watched(&tvdb_only), None);
        assert_eq!(movies_from_watched(&Value::Null), WatchedMovies::default());
    }

    #[test]
    fn numbering_answer_skips_incomplete_episodes() {
        let map = episode_map_from_seasons(&json!([{"number":1,"episodes":[
            {"season":1,"number":1,"ids":{}},
            {"number":2,"ids":{"tvdb":5}},
            {"season":1,"number":3,"ids":{"tvdb":6}}]}]));
        assert_eq!(map.find(&ids(Some(6), None, None)), Some((1, 3)));
        assert_eq!(map.find(&ids(Some(5), None, None)), None);
        assert_eq!(episode_map_from_seasons(&json!({})), EpisodeMap::default());
    }

    #[test]
    fn show_lookup_paths_follow_the_ids() {
        let ids = ShowIds {
            tvdb: Some(5),
            tmdb: Some(6),
            imdb: Some("tt7".into()),
            ..Default::default()
        };
        assert_eq!(
            show_lookups(&ids),
            vec![
                "search/tvdb/5?type=show",
                "search/tmdb/6?type=show",
                "search/imdb/tt7?type=show"
            ]
        );
        assert_eq!(show_key(&ids).as_deref(), Some("tvdb:5"));
        assert_eq!(show_key(&ShowIds::default()), None);
    }
}
