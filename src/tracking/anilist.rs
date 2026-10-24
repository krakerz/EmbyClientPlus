//! AniList: implicit grant (no refresh token, ~1 year), GraphQL over plain POSTs.

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
    TrackingService, Update, http, oauth,
};

pub const AUTHORIZE_URL: &str = "https://anilist.co/api/v2/oauth/authorize";
pub const GRAPHQL_URL: &str = "https://graphql.anilist.co";
pub const CLIENT_ID: &str = "54733";

const ID: ServiceId = ServiceId::AniList;

const READ_QUERY: &str = "query($id:Int){Media(id:$id,type:ANIME){episodes status format mediaListEntry{progress status}}}";
/// Several entries at once, for the sync markers (`$ids` up to `ENTRIES_PER_QUERY`).
const ENTRIES_QUERY: &str = "query($ids:[Int]){Page(perPage:50){media(id_in:$ids,type:ANIME){id episodes status title{userPreferred} mediaListEntry{progress status}}}}";
const ENTRIES_PER_QUERY: usize = 50;
const WRITE_MUTATION: &str = "mutation($mediaId:Int,$progress:Int,$status:MediaListStatus){SaveMediaListEntry(mediaId:$mediaId,progress:$progress,status:$status){id}}";

pub struct AniList {
    tokens: Mutex<Option<Tokens>>,
    refresh_lock: tokio::sync::Mutex<()>,
}

impl Default for AniList {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct RawMedia {
    #[serde(default)]
    episodes: Option<u32>,
    /// The media's release status (`RELEASING`, `FINISHED`, ...).
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "mediaListEntry")]
    entry: Option<RawEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    #[serde(default)]
    progress: u32,
    status: String,
}

#[derive(Debug, PartialEq, Eq)]
struct Anime {
    total: Option<u32>,
    airing: bool,
    entry: Option<Entry>,
}

fn parse_status(status: &str) -> ListStatus {
    match status {
        "CURRENT" => ListStatus::Watching,
        "COMPLETED" => ListStatus::Completed,
        "PLANNING" => ListStatus::PlanToWatch,
        "PAUSED" => ListStatus::OnHold,
        "DROPPED" => ListStatus::Dropped,
        _ => ListStatus::Other, // REPEATING and anything new
    }
}

fn status_str(status: ListStatus) -> &'static str {
    match status {
        ListStatus::Watching | ListStatus::Other => "CURRENT",
        ListStatus::Completed => "COMPLETED",
        ListStatus::PlanToWatch => "PLANNING",
        ListStatus::OnHold => "PAUSED",
        ListStatus::Dropped => "DROPPED",
    }
}

fn parse_anime(data: &Value) -> Result<Anime> {
    let media = data
        .get("Media")
        .filter(|m| !m.is_null())
        .ok_or_else(|| anyhow::anyhow!("AniList: no such anime"))?;
    let raw: RawMedia = serde_json::from_value(media.clone())?;
    Ok(Anime {
        total: raw.episodes.filter(|n| *n > 0),
        airing: raw.status.as_deref() == Some("RELEASING"),
        entry: raw.entry.map(|e| Entry {
            progress: e.progress,
            status: parse_status(&e.status),
        }),
    })
}

/// `Page.media` of an `ENTRIES_QUERY` answer -> entries by AniList id.
fn parse_entries(data: &Value) -> HashMap<u32, EntryInfo> {
    let media = data
        .pointer("/Page/media")
        .and_then(Value::as_array)
        .into_iter()
        .flatten();
    media
        .filter_map(|m| {
            let id = u32::try_from(m.get("id")?.as_u64()?).ok()?;
            let title = m
                .pointer("/title/userPreferred")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let entry = m
                .get("mediaListEntry")
                .filter(|e| !e.is_null())
                .and_then(|e| serde_json::from_value::<RawEntry>(e.clone()).ok())
                .map(|e| Entry {
                    progress: e.progress,
                    status: parse_status(&e.status),
                });
            let total = m
                .get("episodes")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0);
            let airing = m.get("status").and_then(Value::as_str) == Some("RELEASING");
            Some((
                id,
                EntryInfo {
                    title,
                    entry,
                    total,
                    airing,
                },
            ))
        })
        .collect()
}

fn write_variables(id: u32, entry: Entry) -> Value {
    json!({
        "mediaId": id,
        "progress": entry.progress,
        "status": status_str(entry.status),
    })
}

/// `Some(message)` for a failed GraphQL answer; `logged_out` when the token was refused.
fn graphql_error(status: u16, body: &Value) -> Option<(String, bool)> {
    let first = body
        .get("errors")
        .and_then(Value::as_array)
        .filter(|errors| !errors.is_empty())
        .and_then(|errors| errors.first())?;
    let message = first
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown error")
        .to_string();
    let logged_out = message.contains("Invalid token") || status == 401;
    Some((message, logged_out))
}

impl AniList {
    pub fn new() -> Self {
        AniList {
            tokens: Mutex::new(tokens::load(ID)),
            refresh_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Tokens>> {
        self.tokens.lock().unwrap_or_else(|e| e.into_inner())
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

    /// No refresh grant: an expired token means logging in again.
    async fn renew(&self) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        let Some(current) = self.lock().clone() else {
            return Err(LoggedOut(ID).into());
        };
        if current.expires_within(0) {
            self.drop_tokens();
            return Err(LoggedOut(ID).into());
        }
        Ok(())
    }

    async fn post(&self, access: &str, query: &str, variables: &Value) -> Result<Value> {
        let resp = http()
            .post(GRAPHQL_URL)
            .bearer_auth(access)
            .header("Accept", "application/json")
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if let Some((message, logged_out)) = graphql_error(status.as_u16(), &body) {
            if logged_out {
                self.drop_tokens();
                return Err(LoggedOut(ID).into());
            }
            bail!("AniList {status}: {message}");
        }
        if status.as_u16() == 401 {
            self.drop_tokens();
            return Err(LoggedOut(ID).into());
        }
        if !status.is_success() {
            bail!(
                "AniList {status}: {}",
                text.chars().take(200).collect::<String>()
            );
        }
        Ok(body.get("data").cloned().unwrap_or(Value::Null))
    }

    /// POSTs a GraphQL document and returns its `data`.
    async fn gql(&self, query: &str, variables: Value) -> Result<Value> {
        self.renew().await?;
        let access = self.access()?;
        self.post(&access, query, &variables).await
    }

    async fn login_browser(&self) -> Result<Account> {
        let state = oauth::random_token();
        let listener = oauth::Listener::bind()?;
        let url = reqwest::Url::parse_with_params(
            AUTHORIZE_URL,
            [
                ("client_id", CLIENT_ID),
                ("response_type", "token"),
                ("state", state.as_str()),
            ],
        )?;
        oauth::open_browser(url.into());
        let wait_state = state.clone();
        let params = tokio::task::spawn_blocking(move || {
            listener.wait("anilist", &wait_state, oauth::TIMEOUT)
        })
        .await??;
        let Some(access) = params.get("access_token") else {
            bail!(
                "AniList login failed: {}",
                params
                    .get("error")
                    .map_or("no token returned", String::as_str)
            );
        };
        let expires_in = params.get("expires_in").and_then(|v| v.parse::<i64>().ok());
        let data = self
            .post(access, "query { Viewer { id name } }", &json!({}))
            .await?;
        let name = data
            .pointer("/Viewer/name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("AniList: no viewer in the answer"))?
            .to_string();
        let new = Tokens {
            access: access.clone(),
            refresh: None,
            expires_at: tokens::expires_at(expires_in),
            username: name.clone(),
            user_id: data
                .pointer("/Viewer/id")
                .and_then(Value::as_u64)
                .map(|id| id.to_string()),
        };
        tokens::save(ID, &new)?;
        *self.lock() = Some(new);
        Ok(Account { name })
    }

    async fn read(&self, id: u32) -> Result<Anime> {
        let data = self.gql(READ_QUERY, json!({ "id": id })).await?;
        parse_anime(&data)
    }

    async fn write(&self, id: u32, entry: Entry) -> Result<()> {
        self.gql(WRITE_MUTATION, write_variables(id, entry)).await?;
        Ok(())
    }
}

impl TrackingService for AniList {
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
                _ => bail!("AniList logs in through the browser"),
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
        Box::pin(async move { self.renew().await })
    }

    fn entries<'a>(
        &'a self,
        keys: &'a [EntryKey],
    ) -> BoxFuture<'a, Result<HashMap<u32, EntryInfo>>> {
        Box::pin(async move {
            let ids: Vec<u32> = keys.iter().map(|key| key.id).collect();
            let mut found = HashMap::new();
            for chunk in ids.chunks(ENTRIES_PER_QUERY) {
                let data = self.gql(ENTRIES_QUERY, json!({ "ids": chunk })).await?;
                found.extend(parse_entries(&data));
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
            let Some(t) = target.anilist else {
                tracing::debug!("AniList: no AniList target in the match");
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
        assert_eq!(parse_status("REPEATING"), ListStatus::Other);
    }

    #[test]
    fn parses_media_with_and_without_entry() {
        let data = json!({"Media":{"episodes":12,"format":"TV","status":"RELEASING",
            "mediaListEntry":{"progress":4,"status":"CURRENT"}}});
        assert_eq!(
            parse_anime(&data).unwrap(),
            Anime {
                total: Some(12),
                airing: true,
                entry: Some(Entry {
                    progress: 4,
                    status: ListStatus::Watching
                })
            }
        );
        let data = json!({"Media":{"episodes":null,"format":"TV","mediaListEntry":null}});
        assert_eq!(
            parse_anime(&data).unwrap(),
            Anime {
                total: None,
                airing: false,
                entry: None
            }
        );
        assert!(parse_anime(&json!({"Media":null})).is_err());
    }

    #[test]
    fn write_variables_shape() {
        let v = write_variables(
            189117,
            Entry {
                progress: 1,
                status: ListStatus::Watching,
            },
        );
        assert_eq!(v, json!({"mediaId":189117,"progress":1,"status":"CURRENT"}));
    }

    #[test]
    fn graphql_errors() {
        let body = json!({"errors":[{"message":"Invalid token"}]});
        assert_eq!(
            graphql_error(400, &body),
            Some(("Invalid token".to_string(), true))
        );
        let body = json!({"errors":[{"message":"Not Found."}]});
        assert_eq!(
            graphql_error(404, &body),
            Some(("Not Found.".to_string(), false))
        );
        assert_eq!(graphql_error(200, &json!({"data":{}})), None);
    }

    #[test]
    fn parses_batched_entries() {
        let data = json!({"Page":{"media":[
            {"id":1,"episodes":12,"status":"RELEASING","title":{"userPreferred":"A"},"mediaListEntry":{"progress":3,"status":"CURRENT"}},
            {"id":2,"title":{"userPreferred":"B"},"mediaListEntry":null}]}});
        let found = parse_entries(&data);
        assert_eq!(found.len(), 2);
        assert_eq!(found[&1].title, "A");
        assert_eq!(found[&1].total, Some(12));
        assert!(found[&1].airing);
        assert_eq!(found[&2].total, None);
        assert!(!found[&2].airing);
        assert_eq!(
            found[&1].entry,
            Some(Entry {
                progress: 3,
                status: ListStatus::Watching
            })
        );
        assert_eq!(found[&2].entry, None);
        assert!(parse_entries(&json!({})).is_empty());
    }
}
