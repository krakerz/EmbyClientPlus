pub mod auth;
pub mod library;
pub mod models;
pub mod playback_info;
pub mod sessions;

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue};

use models::ItemDto;

const DEVICE_NAME: &str = "EmbyClientPlus";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct EmbyClient {
    base_url: String,
    http: reqwest::Client,
    device_id: String,
    /// Set once authenticated (either via `auth::authenticate_by_name`, or
    /// by the caller after reading the token out of the embedded webview's
    /// localStorage — the primary path per the app's design).
    access_token: Option<String>,
}

impl EmbyClient {
    pub fn new(base_url: impl Into<String>, device_id: impl Into<String>) -> Self {
        EmbyClient {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
            device_id: device_id.into(),
            access_token: None,
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.access_token = Some(token.into());
        self
    }

    #[allow(dead_code)] // only used by the currently-unwired auth::authenticate_by_name fallback
    pub fn set_token(&mut self, token: impl Into<String>) {
        self.access_token = Some(token.into());
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// The `X-Emby-Authorization` header Emby expects on every request,
    /// authenticated or not — client/device identification is separate
    /// from the bearer token itself.
    fn emby_authorization_header(&self) -> String {
        format!(
            "MediaBrowser Client=\"{DEVICE_NAME}\", Device=\"{DEVICE_NAME}\", DeviceId=\"{}\", Version=\"{APP_VERSION}\"",
            self.device_id
        )
    }

    fn headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Emby-Authorization",
            HeaderValue::from_str(&self.emby_authorization_header())
                .context("invalid characters in emby authorization header")?,
        );
        if let Some(token) = &self.access_token {
            headers.insert(
                "X-Emby-Token",
                HeaderValue::from_str(token).context("invalid characters in access token")?,
            );
        }
        Ok(headers)
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self
            .http
            .get(self.url(path))
            .headers(self.headers()?)
            .send()
            .await
            .with_context(|| format!("GET {path} failed"))?
            .error_for_status()
            .with_context(|| format!("GET {path} returned an error status"))?;
        response
            .json()
            .await
            .with_context(|| format!("GET {path} returned an unparsable body"))
    }

    async fn get_text(&self, path: &str) -> Result<String> {
        let response = self
            .http
            .get(self.url(path))
            .headers(self.headers()?)
            .send()
            .await
            .with_context(|| format!("GET {path} failed"))?
            .error_for_status()
            .with_context(|| format!("GET {path} returned an error status"))?;
        response
            .text()
            .await
            .with_context(|| format!("GET {path} returned an unreadable body"))
    }

    async fn post<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let response = self
            .http
            .post(self.url(path))
            .headers(self.headers()?)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {path} failed"))?
            .error_for_status()
            .with_context(|| format!("POST {path} returned an error status"))?;
        response
            .json()
            .await
            .with_context(|| format!("POST {path} returned an unparsable body"))
    }

    /// Fire-and-forget POST for session reporting endpoints, which reply
    /// with an empty body on success.
    async fn post_empty<B: serde::Serialize>(&self, path: &str, body: &B) -> Result<()> {
        self.http
            .post(self.url(path))
            .headers(self.headers()?)
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {path} failed"))?
            .error_for_status()
            .with_context(|| format!("POST {path} returned an error status"))?;
        Ok(())
    }

    /// A direct-play stream URL for an already-negotiated media source.
    /// Interpolation happens after decode either way (see project
    /// CLAUDE.md), so this is deliberately decoupled from whatever
    /// direct-play-vs-transcode PlaybackInfo negotiated — mpv just needs
    /// something it can open.
    pub fn direct_stream_url(&self, item_id: &str, media_source_id: &str) -> Result<String> {
        let token = self
            .access_token
            .as_deref()
            .context("cannot build a stream url before authentication")?;
        Ok(format!(
            "{}/emby/Videos/{item_id}/stream?static=true&mediaSourceId={media_source_id}&api_key={token}",
            self.base_url
        ))
    }

    /// Resolves a `TranscodingUrl` from a PlaybackInfo response into a
    /// fully-qualified URL mpv can open directly — Emby returns these as
    /// server-root-relative paths (already carrying whatever query params
    /// authenticate/identify the transcode session), so this only needs
    /// to prefix `base_url` when the path isn't already absolute.
    pub fn resolve_transcoding_url(&self, transcoding_url: &str) -> String {
        if transcoding_url.starts_with("http://") || transcoding_url.starts_with("https://") {
            transcoding_url.to_string()
        } else {
            format!("{}{}", self.base_url, transcoding_url)
        }
    }

    /// Fetches a subtitle stream's raw content — works for embedded or
    /// external tracks alike, since Emby serves any subtitle stream as a
    /// standalone file through this endpoint regardless of how it was
    /// originally delivered.
    pub async fn get_subtitle_stream(
        &self,
        item_id: &str,
        media_source_id: &str,
        stream_index: i32,
        format: &str,
    ) -> Result<String> {
        self.get_text(&format!(
            "/emby/Videos/{item_id}/{media_source_id}/Subtitles/{stream_index}/Stream.{format}"
        ))
        .await
    }

    /// Resolves an item's parent `SeriesId` (for episodes) so per-title
    /// overrides can be keyed at the series level. Returns `None` for
    /// items with no series (movies use their own ItemId directly).
    pub async fn get_item_series_id(&self, user_id: &str, item_id: &str) -> Result<Option<String>> {
        let item: ItemDto = self
            .get(&format!(
                "/emby/Users/{user_id}/Items/{item_id}?Fields=SeriesId"
            ))
            .await?;
        Ok(item.series_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_trailing_slash_is_stripped() {
        let client = EmbyClient::new("http://192.168.1.3:8096/", "device-1");
        assert_eq!(
            client.url("/emby/System/Info"),
            "http://192.168.1.3:8096/emby/System/Info"
        );
    }

    #[test]
    fn emby_authorization_header_includes_device_id() {
        let client = EmbyClient::new("http://server", "my-device-id");
        let header = client.emby_authorization_header();
        assert!(header.contains("DeviceId=\"my-device-id\""));
        assert!(header.contains("Client=\"EmbyClientPlus\""));
    }

    #[test]
    fn headers_omit_token_before_authentication() {
        let client = EmbyClient::new("http://server", "device-1");
        let headers = client.headers().unwrap();
        assert!(headers.get("X-Emby-Token").is_none());
    }

    #[test]
    fn headers_include_token_after_set() {
        let client = EmbyClient::new("http://server", "device-1").with_token("secret-token");
        let headers = client.headers().unwrap();
        assert_eq!(headers.get("X-Emby-Token").unwrap(), "secret-token");
    }

    #[test]
    fn resolve_transcoding_url_prefixes_a_relative_path() {
        let client = EmbyClient::new("http://192.168.1.3:8096", "device-1");
        let resolved = client.resolve_transcoding_url("/videos/123/master.m3u8?MediaSourceId=abc");
        assert_eq!(
            resolved,
            "http://192.168.1.3:8096/videos/123/master.m3u8?MediaSourceId=abc"
        );
    }

    #[test]
    fn resolve_transcoding_url_leaves_an_absolute_url_untouched() {
        let client = EmbyClient::new("http://192.168.1.3:8096", "device-1");
        let absolute = "https://cdn.example.com/stream.m3u8";
        assert_eq!(client.resolve_transcoding_url(absolute), absolute);
    }
}
