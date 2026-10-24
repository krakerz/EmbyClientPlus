//! MyAnimeList API v2: auth code + PKCE `plain`, no secret.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use anyhow::{Result, bail};
use serde::Deserialize;

use super::rules::{self, Entry, ListStatus, Plan};
use super::tokens::{self, Tokens};
use super::{
    Account, BoxFuture, Direction, EntryInfo, EntryKey, LoggedOut, LoginInput, Outcome, ServiceId,
    TrackingService, Update, http, oauth,
};

pub const AUTHORIZE_URL: &str = "https://myanimelist.net/v1/oauth2/authorize";
pub const TOKEN_URL: &str = "https://myanimelist.net/v1/oauth2/token";
pub const API_URL: &str = "https://api.myanimelist.net/v2";
pub const CLIENT_ID: &str = "998eb7339303c6560e4f9dfac109a1e4";

const ID: ServiceId = ServiceId::Mal;
const REFRESH_MARGIN: i64 = 86_400;

pub struct Mal {
    tokens: Mutex<Option<Tokens>>,
    refresh_lock: tokio::sync::Mutex<()>,
}

impl Default for Mal {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[derive(Deserialize, Default)]
struct RawAnime {
    #[serde(default)]
    num_episodes: u32,
    #[serde(default)]
    status: String,
    #[serde(default)]
    my_list_status: Option<RawListStatus>,
}

/// `anime/{id}?fields=title,my_list_status,num_episodes,status`.
#[derive(Deserialize)]
struct RawInfo {
    #[serde(default)]
    title: String,
    #[serde(default)]
    num_episodes: u32,
    #[serde(default)]
    status: String,
    #[serde(default)]
    my_list_status: Option<RawListStatus>,
}

impl RawInfo {
    fn into_info(self) -> EntryInfo {
        EntryInfo {
            title: self.title,
            entry: self.my_list_status.map(|s| Entry {
                progress: s.num_episodes_watched,
                status: parse_status(&s.status),
            }),
            total: (self.num_episodes > 0).then_some(self.num_episodes),
            airing: self.status == "currently_airing",
        }
    }
}

#[derive(Deserialize)]
struct RawListStatus {
    status: String,
    #[serde(default)]
    num_episodes_watched: u32,
}

/// The anime as MAL has it: episode total (None: unknown) and the user's entry.
#[derive(Debug, PartialEq, Eq)]
struct Anime {
    total: Option<u32>,
    airing: bool,
    entry: Option<Entry>,
}

impl RawAnime {
    fn into_anime(self) -> Anime {
        Anime {
            total: (self.num_episodes > 0).then_some(self.num_episodes),
            airing: self.status == "currently_airing",
            entry: self.my_list_status.map(|s| Entry {
                progress: s.num_episodes_watched,
                status: parse_status(&s.status),
            }),
        }
    }
}

pub(super) fn parse_status(status: &str) -> ListStatus {
    match status {
        "watching" => ListStatus::Watching,
        "completed" => ListStatus::Completed,
        "on_hold" => ListStatus::OnHold,
        "dropped" => ListStatus::Dropped,
        "plan_to_watch" => ListStatus::PlanToWatch,
        _ => ListStatus::Other,
    }
}

fn status_str(status: ListStatus) -> &'static str {
    match status {
        ListStatus::Watching | ListStatus::Other => "watching",
        ListStatus::Completed => "completed",
        ListStatus::PlanToWatch => "plan_to_watch",
        ListStatus::OnHold => "on_hold",
        ListStatus::Dropped => "dropped",
    }
}

/// The write the list rules ask for (shared by MAL, AniList and Kitsu).
pub(super) fn plan(
    current: Option<Entry>,
    total: Option<u32>,
    episode: u32,
    direction: Direction,
    movie: bool,
) -> Plan {
    match (movie, direction) {
        (false, Direction::Watched) => rules::watched(current, episode, total),
        (false, Direction::Unwatched) => rules::unwatched(current, episode),
        (true, Direction::Watched) => rules::movie_watched(current),
        (true, Direction::Unwatched) => rules::movie_unwatched(current),
    }
}

fn write_form(entry: Entry) -> [(&'static str, String); 2] {
    [
        ("status", status_str(entry.status).to_string()),
        ("num_watched_episodes", entry.progress.to_string()),
    ]
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

impl Mal {
    pub fn new() -> Self {
        Mal {
            tokens: Mutex::new(tokens::load(ID)),
            refresh_lock: tokio::sync::Mutex::new(()),
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

    /// Refreshes under the lock. `stale` is the access token a 401 came back
    /// for: when the stored one differs, someone else already renewed it.
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
        let resp = http()
            .post(TOKEN_URL)
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
            ])
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

    async fn send(&self, build: impl Fn() -> reqwest::RequestBuilder) -> Result<reqwest::Response> {
        self.renew(None).await?;
        let access = self.access()?;
        let mut resp = build().bearer_auth(&access).send().await?;
        if resp.status().as_u16() == 401 {
            self.renew(Some(&access)).await?;
            let access = self.access()?;
            resp = build().bearer_auth(&access).send().await?;
            if resp.status().as_u16() == 401 {
                self.drop_tokens();
                return Err(LoggedOut(ID).into());
            }
        }
        check(resp).await
    }

    async fn login_browser(&self) -> Result<Account> {
        let state = oauth::random_token();
        let verifier = oauth::random_token();
        let redirect = oauth::redirect_uri("mal");
        let listener = oauth::Listener::bind()?;
        let url = reqwest::Url::parse_with_params(
            AUTHORIZE_URL,
            [
                ("response_type", "code"),
                ("client_id", CLIENT_ID),
                ("state", state.as_str()),
                ("redirect_uri", redirect.as_str()),
                ("code_challenge", verifier.as_str()),
                ("code_challenge_method", "plain"),
            ],
        )?;
        oauth::open_browser(url.into());
        let wait_state = state.clone();
        let params =
            tokio::task::spawn_blocking(move || listener.wait("mal", &wait_state, oauth::TIMEOUT))
                .await??;
        let Some(code) = params.get("code") else {
            bail!(
                "MyAnimeList login failed: {}",
                params
                    .get("error")
                    .map_or("no code returned", String::as_str)
            );
        };
        let resp = http()
            .post(TOKEN_URL)
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", redirect.as_str()),
                ("code_verifier", verifier.as_str()),
            ])
            .send()
            .await?;
        let body: TokenResponse = check(resp).await?.json().await?;
        #[derive(Deserialize)]
        struct Me {
            name: String,
        }
        let me: Me = check(
            http()
                .get(format!("{API_URL}/users/@me"))
                .bearer_auth(&body.access_token)
                .send()
                .await?,
        )
        .await?
        .json()
        .await?;
        let new = Tokens {
            access: body.access_token,
            refresh: body.refresh_token,
            expires_at: tokens::expires_at(body.expires_in),
            username: me.name.clone(),
            user_id: None,
        };
        tokens::save(ID, &new)?;
        *self.lock() = Some(new);
        Ok(Account { name: me.name })
    }

    async fn read(&self, id: u32) -> Result<Anime> {
        let url = format!("{API_URL}/anime/{id}");
        let raw: RawAnime = self
            .send(|| {
                http()
                    .get(&url)
                    .query(&[("fields", "num_episodes,status,my_list_status")])
            })
            .await?
            .json()
            .await?;
        Ok(raw.into_anime())
    }

    /// Title and the user's entry, for the sync markers.
    async fn info(&self, id: u32) -> Result<EntryInfo> {
        let url = format!("{API_URL}/anime/{id}");
        let raw: RawInfo = self
            .send(|| {
                http()
                    .get(&url)
                    .query(&[("fields", "title,num_episodes,status,my_list_status")])
            })
            .await?
            .json()
            .await?;
        Ok(raw.into_info())
    }

    async fn write(&self, id: u32, entry: Entry) -> Result<()> {
        let url = format!("{API_URL}/anime/{id}/my_list_status");
        let form = write_form(entry);
        self.send(|| http().patch(&url).form(&form)).await?;
        Ok(())
    }
}

impl TrackingService for Mal {
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
                LoginInput::Browser => self.login_browser().await,
                _ => bail!("MyAnimeList logs in through the browser"),
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

    fn entries<'a>(
        &'a self,
        keys: &'a [EntryKey],
    ) -> BoxFuture<'a, Result<HashMap<u32, EntryInfo>>> {
        Box::pin(async move {
            let mut found = HashMap::new();
            for key in keys {
                found.insert(key.id, self.info(key.id).await?);
            }
            Ok(found)
        })
    }

    fn update<'a>(&'a self, update: &'a Update) -> BoxFuture<'a, Result<Outcome>> {
        Box::pin(async move {
            let Update::Progress {
                target,
                direction,
                movie,
            } = update
            else {
                return Ok(Outcome::Unchanged);
            };
            let Some(t) = target.mal else {
                tracing::debug!("MyAnimeList: no MAL target in the match");
                return Ok(Outcome::Unchanged);
            };
            let anime = self.read(t.id).await?;
            if t.extrapolated
                && let Some(reason) =
                    rules::extrapolation_skip(anime.total, anime.airing, t.episode)
            {
                return Ok(Outcome::Skipped(reason.to_string()));
            }
            match plan(anime.entry, anime.total, t.episode, *direction, *movie) {
                Plan::Write(entry) => {
                    self.write(t.id, entry).await?;
                    Ok(Outcome::Done)
                }
                Plan::Unchanged => Ok(Outcome::Unchanged),
                Plan::Skip(reason) => Ok(Outcome::Skipped(reason)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_strings_round_trip() {
        for status in [
            ListStatus::Watching,
            ListStatus::Completed,
            ListStatus::PlanToWatch,
            ListStatus::OnHold,
            ListStatus::Dropped,
        ] {
            assert_eq!(parse_status(status_str(status)), status);
        }
        assert_eq!(parse_status("something_new"), ListStatus::Other);
    }

    #[test]
    fn parses_an_anime_with_an_entry() {
        let raw: RawAnime = serde_json::from_str(
            r#"{"id":1,"title":"x","num_episodes":12,"status":"currently_airing",
                "my_list_status":{"status":"watching","num_episodes_watched":5,"score":0}}"#,
        )
        .unwrap();
        assert_eq!(
            raw.into_anime(),
            Anime {
                total: Some(12),
                airing: true,
                entry: Some(Entry {
                    progress: 5,
                    status: ListStatus::Watching
                }),
            }
        );
    }

    #[test]
    fn unknown_total_and_no_entry() {
        let raw: RawAnime = serde_json::from_str(r#"{"id":1,"num_episodes":0}"#).unwrap();
        assert_eq!(
            raw.into_anime(),
            Anime {
                total: None,
                airing: false,
                entry: None
            }
        );
    }

    #[test]
    fn write_form_has_status_and_progress() {
        let form = write_form(Entry {
            progress: 3,
            status: ListStatus::Completed,
        });
        assert_eq!(form[0], ("status", "completed".to_string()));
        assert_eq!(form[1], ("num_watched_episodes", "3".to_string()));
    }

    #[test]
    fn plan_picks_the_movie_rules() {
        assert!(matches!(
            plan(None, None, 1, Direction::Watched, true),
            Plan::Write(Entry {
                progress: 1,
                status: ListStatus::Completed
            })
        ));
        assert_eq!(
            plan(None, Some(12), 3, Direction::Unwatched, false),
            Plan::Unchanged
        );
    }

    #[test]
    fn parses_title_and_entry() {
        let raw: RawInfo = serde_json::from_str(
            r#"{"title":"Dr. Stone","num_episodes":0,"status":"currently_airing",
                "my_list_status":{"status":"watching","num_episodes_watched":4}}"#,
        )
        .unwrap();
        let info = raw.into_info();
        assert_eq!(info.title, "Dr. Stone");
        assert_eq!(info.total, None);
        assert!(info.airing);
        assert_eq!(
            info.entry,
            Some(Entry {
                progress: 4,
                status: ListStatus::Watching
            })
        );
        let bare: RawInfo = serde_json::from_str(r#"{"title":"X"}"#).unwrap();
        assert_eq!(bare.into_info().entry, None);
    }
}
