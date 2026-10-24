//! Kitsu: password grant, no registration. `kitsu.app` is the
//! current domain (`kitsu.io` still answers); Cloudflare rejects requests
//! without a User-Agent and form content type.
//!
//! The password grant is sent without client credentials, as the plan has
//! it (the public client id/secret from Kitsu's docs could not be confirmed
//! from the API docs or kitsu-server, so they are not included).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use super::mal::plan;
use super::rules::{self, Entry, ListStatus, Plan};
use super::tokens::{self, Tokens};
use super::{
    Account, BoxFuture, EntryInfo, EntryKey, LoggedOut, LoginInput, Outcome, ServiceId,
    TrackingService, Update, http,
};

pub const API_URL: &str = "https://kitsu.app/api/edge";
pub const TOKEN_URL: &str = "https://kitsu.app/api/oauth/token";

const ID: ServiceId = ServiceId::Kitsu;
const REFRESH_MARGIN: i64 = 86_400;
const JSON_API: &str = "application/vnd.api+json";
/// `mappings` sites: Kitsu often has no MAL mapping for a new show, but an AniList one.
const MAL_SITE: &str = "myanimelist/anime";
const ANILIST_SITE: &str = "anilist/anime";

pub struct Kitsu {
    tokens: Mutex<Option<Tokens>>,
    refresh_lock: tokio::sync::Mutex<()>,
    /// (mappings site, external id) -> the Kitsu anime; hits only.
    kitsu_ids: Mutex<HashMap<(&'static str, u32), KitsuAnime>>,
}

/// A Kitsu anime found through a mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KitsuAnime {
    id: String,
    total: Option<u32>,
    /// Kitsu's status is `current`.
    airing: bool,
    title: String,
}

impl Default for Kitsu {
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

/// A library entry as Kitsu has it.
#[derive(Debug, PartialEq, Eq)]
struct Library {
    id: String,
    entry: Entry,
}

fn parse_status(status: &str) -> ListStatus {
    match status {
        "current" => ListStatus::Watching,
        "completed" => ListStatus::Completed,
        "planned" => ListStatus::PlanToWatch,
        "on_hold" => ListStatus::OnHold,
        "dropped" => ListStatus::Dropped,
        _ => ListStatus::Other,
    }
}

fn status_str(status: ListStatus) -> &'static str {
    match status {
        ListStatus::Watching | ListStatus::Other => "current",
        ListStatus::Completed => "completed",
        ListStatus::PlanToWatch => "planned",
        ListStatus::OnHold => "on_hold",
        ListStatus::Dropped => "dropped",
    }
}

/// `mappings?...&include=item` -> the Kitsu anime.
fn parse_mapping(body: &Value) -> Option<KitsuAnime> {
    let item = body.get("included")?.as_array()?.first()?;
    let id = item.get("id")?.as_str()?.to_string();
    let total = item
        .pointer("/attributes/episodeCount")
        .and_then(Value::as_u64)
        .map(|n| n as u32)
        .filter(|n| *n > 0);
    Some(KitsuAnime {
        id,
        total,
        airing: item.pointer("/attributes/status").and_then(Value::as_str) == Some("current"),
        title: parse_title(body).unwrap_or_default(),
    })
}

/// The anime's title in a `mappings?...&include=item` answer.
fn parse_title(body: &Value) -> Option<String> {
    let item = body.get("included")?.as_array()?.first()?;
    ["canonicalTitle", "titles/en_jp", "titles/en"]
        .iter()
        .find_map(|key| {
            item.pointer(&format!("/attributes/{key}"))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
}

/// `library-entries?filter[userId]&filter[animeId]` -> the first entry.
fn parse_library(body: &Value) -> Option<Library> {
    let first = body.get("data")?.as_array()?.first()?;
    Some(Library {
        id: first.get("id")?.as_str()?.to_string(),
        entry: Entry {
            progress: first
                .pointer("/attributes/progress")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            status: parse_status(
                first
                    .pointer("/attributes/status")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            ),
        },
    })
}

fn patch_body(id: &str, entry: Entry) -> Value {
    json!({"data":{"id":id,"type":"libraryEntries","attributes":{
        "progress":entry.progress,"status":status_str(entry.status)}}})
}

fn create_body(user: &str, anime: &str, entry: Entry) -> Value {
    json!({"data":{"type":"libraryEntries",
        "attributes":{"progress":entry.progress,"status":status_str(entry.status)},
        "relationships":{
            "user":{"data":{"type":"users","id":user}},
            "anime":{"data":{"type":"anime","id":anime}}}}})
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

impl Kitsu {
    pub fn new() -> Self {
        Kitsu {
            tokens: Mutex::new(tokens::load(ID)),
            refresh_lock: tokio::sync::Mutex::new(()),
            kitsu_ids: Mutex::new(HashMap::new()),
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

    async fn login_password(&self, username: String, password: String) -> Result<Account> {
        let resp = http()
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "password"),
                ("username", username.as_str()),
                ("password", password.as_str()),
            ])
            .send()
            .await?;
        drop(password);
        if matches!(resp.status().as_u16(), 400 | 401) {
            bail!("Wrong email/username or password");
        }
        let body: TokenResponse = check(resp).await?.json().await?;
        let me: Value = check(
            http()
                .get(format!("{API_URL}/users"))
                .query(&[("filter[self]", "true")])
                .header("Accept", JSON_API)
                .bearer_auth(&body.access_token)
                .send()
                .await?,
        )
        .await?
        .json()
        .await?;
        let user = me
            .pointer("/data/0")
            .ok_or_else(|| anyhow::anyhow!("Kitsu: no user in the answer"))?;
        let id = user
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("Kitsu: no user id in the answer"))?
            .to_string();
        let name = user
            .pointer("/attributes/name")
            .and_then(Value::as_str)
            .unwrap_or(&username)
            .to_string();
        let new = Tokens {
            access: body.access_token,
            refresh: body.refresh_token,
            expires_at: tokens::expires_at(body.expires_in),
            username: name.clone(),
            user_id: Some(id),
        };
        tokens::save(ID, &new)?;
        *self.lock() = Some(new);
        Ok(Account { name })
    }

    /// The Kitsu anime of a MAL id, else of the AniList id of the same show
    /// (cached under both; misses aren't).
    async fn kitsu_anime(&self, mal: u32, anilist: Option<u32>) -> Result<Option<KitsuAnime>> {
        let sources: Vec<(&'static str, u32)> = [(MAL_SITE, Some(mal)), (ANILIST_SITE, anilist)]
            .into_iter()
            .filter_map(|(site, id)| Some((site, id?)))
            .collect();
        {
            let cache = self.kitsu_ids.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(hit) = sources.iter().find_map(|source| cache.get(source)) {
                return Ok(Some(hit.clone()));
            }
        }
        for (site, id) in sources {
            let external = id.to_string();
            let body: Value = self
                .send(|| {
                    http()
                        .get(format!("{API_URL}/mappings"))
                        .header("Accept", JSON_API)
                        .query(&[
                            ("filter[externalSite]", site),
                            ("filter[externalId]", external.as_str()),
                            ("include", "item"),
                        ])
                })
                .await?
                .json()
                .await?;
            if let Some(found) = parse_mapping(&body) {
                let mut cache = self.kitsu_ids.lock().unwrap_or_else(|e| e.into_inner());
                cache.insert((site, id), found.clone());
                cache.insert((MAL_SITE, mal), found.clone());
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    async fn read(&self, user: &str, anime: &str) -> Result<Option<Library>> {
        let body: Value = self
            .send(|| {
                http()
                    .get(format!("{API_URL}/library-entries"))
                    .header("Accept", JSON_API)
                    .query(&[("filter[userId]", user), ("filter[animeId]", anime)])
            })
            .await?
            .json()
            .await?;
        Ok(parse_library(&body))
    }

    async fn write(
        &self,
        user: &str,
        anime: &str,
        existing: Option<&Library>,
        entry: Entry,
    ) -> Result<()> {
        let text = match existing {
            Some(lib) => patch_body(&lib.id, entry),
            None => create_body(user, anime, entry),
        }
        .to_string();
        self.send(|| {
            let builder = match existing {
                Some(lib) => http().patch(format!("{API_URL}/library-entries/{}", lib.id)),
                None => http().post(format!("{API_URL}/library-entries")),
            };
            builder
                .header("Content-Type", JSON_API)
                .header("Accept", JSON_API)
                .body(text.clone())
        })
        .await?;
        Ok(())
    }
}

impl TrackingService for Kitsu {
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
                LoginInput::Password { username, password } => {
                    self.login_password(username, password).await
                }
                _ => bail!("Kitsu logs in with a username and password"),
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
            let Some(user) = self.snapshot().and_then(|tk| tk.user_id) else {
                self.drop_tokens();
                return Err(LoggedOut(ID).into());
            };
            let mut found = HashMap::new();
            for key in keys {
                let Some(anime) = self.kitsu_anime(key.id, key.anilist).await? else {
                    continue;
                };
                let existing = self.read(&user, &anime.id).await?;
                found.insert(
                    key.id,
                    EntryInfo {
                        title: anime.title,
                        entry: existing.map(|lib| lib.entry),
                        total: anime.total,
                        airing: anime.airing,
                    },
                );
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
                tracing::debug!("Kitsu: no MAL target in the match");
                return Ok(Outcome::Unchanged);
            };
            let Some(user) = self.snapshot().and_then(|tk| tk.user_id) else {
                self.drop_tokens();
                return Err(LoggedOut(ID).into());
            };
            let Some(anime) = self.kitsu_anime(t.id, target.anilist.map(|a| a.id)).await? else {
                return Ok(Outcome::Skipped("not on Kitsu".to_string()));
            };
            if t.extrapolated
                && let Some(reason) =
                    rules::extrapolation_skip(anime.total, anime.airing, t.episode)
            {
                return Ok(Outcome::Skipped(reason.to_string()));
            }
            let existing = self.read(&user, &anime.id).await?;
            let current = existing.as_ref().map(|lib| lib.entry);
            match plan(current, anime.total, t.episode, *direction, *movie) {
                Plan::Write(entry) => {
                    self.write(&user, &anime.id, existing.as_ref(), entry)
                        .await?;
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
        assert_eq!(parse_status("whatever"), ListStatus::Other);
    }

    #[test]
    fn parses_a_mapping() {
        let body = json!({"data":[{"id":"1"}],
            "included":[{"id":"46170","type":"anime","attributes":{
                "episodeCount":12,"status":"finished","canonicalTitle":"Dr. Stone"}}]});
        assert_eq!(
            parse_mapping(&body),
            Some(KitsuAnime {
                id: "46170".to_string(),
                total: Some(12),
                airing: false,
                title: "Dr. Stone".to_string(),
            })
        );
        let body = json!({"data":[],"included":[{"id":"7","attributes":{
            "episodeCount":null,"status":"current"}}]});
        assert_eq!(
            parse_mapping(&body),
            Some(KitsuAnime {
                id: "7".to_string(),
                total: None,
                airing: true,
                title: String::new(),
            })
        );
        assert_eq!(parse_mapping(&json!({"data":[]})), None);
    }

    #[test]
    fn parses_a_library_entry() {
        let body = json!({"data":[{"id":"99","attributes":{"progress":4,"status":"current"}}]});
        assert_eq!(
            parse_library(&body),
            Some(Library {
                id: "99".to_string(),
                entry: Entry {
                    progress: 4,
                    status: ListStatus::Watching
                }
            })
        );
        assert_eq!(parse_library(&json!({"data":[]})), None);
    }

    #[test]
    fn request_bodies() {
        let entry = Entry {
            progress: 2,
            status: ListStatus::Completed,
        };
        assert_eq!(
            patch_body("99", entry),
            json!({"data":{"id":"99","type":"libraryEntries",
                "attributes":{"progress":2,"status":"completed"}}})
        );
        let create = create_body("5", "46170", entry);
        assert_eq!(create["data"]["relationships"]["user"]["data"]["id"], "5");
        assert_eq!(
            create["data"]["relationships"]["anime"]["data"]["type"],
            "anime"
        );
        assert_eq!(create["data"]["attributes"]["progress"], 2);
    }

    #[test]
    fn parses_a_title() {
        let body = json!({"included":[{"id":"7","attributes":{"canonicalTitle":"Dr. Stone","episodeCount":24}}]});
        assert_eq!(parse_title(&body).as_deref(), Some("Dr. Stone"));
        assert_eq!(parse_title(&json!({})), None);
    }
}
