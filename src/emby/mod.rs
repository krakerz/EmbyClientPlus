pub mod auth;
pub mod browse;
pub mod library;
pub mod models;
pub mod playback_info;
pub mod playlists;
pub mod sessions;
pub mod trickplay;

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue};

const CLIENT_NAME: &str = crate::APP_NAME;

/// How a browser session names itself to Emby (Emby's web app sends
/// "Emby Web" and the browser's name).
const BROWSER_CLIENT: &str = "Emby Web";
const BROWSER_DEVICE: &str = "Firefox";

static APPEAR_AS_BROWSER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether requests identify as a web browser (Preferences → Server);
/// read from config at startup, changed live by Preferences.
pub fn set_appear_as_browser(on: bool) {
    APPEAR_AS_BROWSER.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Client and device names for the authorization header.
fn identity() -> (&'static str, &'static str) {
    if APPEAR_AS_BROWSER.load(std::sync::atomic::Ordering::Relaxed) {
        (BROWSER_CLIENT, BROWSER_DEVICE)
    } else {
        (CLIENT_NAME, device_name())
    }
}

/// What the server's Devices page calls this machine: its host name, like
/// Emby's own apps do, so several computers can be told apart.
fn device_name() -> &'static str {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        host_name()
            .map(|name| name.trim().replace('"', ""))
            .filter(|name| !name.is_empty() && name.is_ascii())
            .unwrap_or_else(|| CLIENT_NAME.to_string())
    })
}

#[cfg(unix)]
fn host_name() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `buf.len()` bytes into `buf`.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).into_owned())
}

#[cfg(windows)]
fn host_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct EmbyClient {
    base_url: String,
    http: reqwest::Client,
    device_id: String,
    /// Set once authenticated, via `authenticate_by_name` or a stored token.
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

    pub fn set_token(&mut self, token: impl Into<String>) {
        self.access_token = Some(token.into());
    }

    /// Absolute URL for a server path.
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// The `X-Emby-Authorization` header Emby expects on every request,
    /// authenticated or not — client/device identification is separate
    /// from the bearer token itself.
    fn emby_authorization_header(&self) -> String {
        let (client, device) = identity();
        format!(
            "MediaBrowser Client=\"{client}\", Device=\"{device}\", DeviceId=\"{}\", Version=\"{APP_VERSION}\"",
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

    async fn delete(&self, path: &str) -> Result<()> {
        self.http
            .delete(self.url(path))
            .headers(self.headers()?)
            .send()
            .await
            .with_context(|| format!("DELETE {path} failed"))?
            .error_for_status()
            .with_context(|| format!("DELETE {path} returned an error status"))?;
        Ok(())
    }

    /// Raw response body, e.g. image bytes.
    pub async fn fetch_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let response = self
            .http
            .get(self.url(path))
            .headers(self.headers()?)
            .send()
            .await
            .with_context(|| format!("GET {path} failed"))?
            .error_for_status()
            .with_context(|| format!("GET {path} returned an error status"))?;
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("GET {path} returned an unreadable body"))?;
        Ok(bytes.to_vec())
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

    /// An item as Emby sends it (kept beside a download as its metadata).
    pub async fn item_json(&self, user_id: &str, item_id: &str) -> Result<String> {
        self.get_text(&format!(
            "/emby/Users/{user_id}/Items/{item_id}?Fields=Chapters,Overview,PremiereDate,DateCreated,MediaSources,ProductionYear,Genres"
        ))
        .await
    }

    /// Streams `url` into `file`, counting bytes into `progress`; stops
    /// early (with an error) once `cancelled` turns true.
    pub async fn download_to(
        &self,
        url: &str,
        file: &std::path::Path,
        progress: &crate::update::Progress,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<()> {
        use std::io::Write;
        let mut response = self
            .http
            .get(url)
            .headers(self.headers()?)
            .send()
            .await
            .context("download failed")?
            .error_for_status()
            .context("download failed")?;
        progress.start(response.content_length().unwrap_or(0));
        let mut out = std::io::BufWriter::new(
            std::fs::File::create(file)
                .with_context(|| format!("couldn't write {}", file.display()))?,
        );
        while let Some(chunk) = response.chunk().await.context("download interrupted")? {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("download cancelled");
            }
            out.write_all(&chunk)
                .with_context(|| format!("couldn't write {}", file.display()))?;
            progress.add(chunk.len() as u64);
        }
        out.flush()
            .with_context(|| format!("couldn't write {}", file.display()))
    }

    /// A subtitle stream as a standalone file mpv can load with `sub-add`;
    /// works for embedded and external tracks alike.
    pub fn subtitle_url(
        &self,
        item_id: &str,
        media_source_id: &str,
        stream_index: i32,
        format: &str,
    ) -> Result<String> {
        let token = self
            .access_token
            .as_deref()
            .context("cannot build a subtitle url before authentication")?;
        Ok(format!(
            "{}/emby/Videos/{item_id}/{media_source_id}/Subtitles/{stream_index}/Stream.{format}?api_key={token}",
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
    #[allow(dead_code)] // Phase 2: own subtitle layer (TODO.md)
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
}

/// Whether a request failed because the server rejected our token.
pub fn is_unauthorized(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<reqwest::Error>()
        .and_then(reqwest::Error::status)
        .is_some_and(|status| status == reqwest::StatusCode::UNAUTHORIZED)
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
        // Both identities in one test: the switch is process-wide.
        set_appear_as_browser(false);
        let app = client.emby_authorization_header();
        set_appear_as_browser(true);
        let browser = client.emby_authorization_header();
        assert!(app.contains("Client=\"Emby Client+\""));
        assert!(!app.contains("Device=\"Firefox\""));
        assert!(browser.contains("Client=\"Emby Web\", Device=\"Firefox\""));
        assert!(browser.contains("DeviceId=\"my-device-id\""));
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
