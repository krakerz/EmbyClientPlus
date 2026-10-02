//! Web video links (Emby's trailers are YouTube pages): resolved to a
//! stream mpv can open with yt-dlp, when it's installed. mpv's own
//! YouTube support needs Lua scripting, which our libmpv leaves out.

use std::process::Command;

use anyhow::{Context, Result, bail};

/// The link needs yt-dlp, which isn't installed; carries the page URL so
/// the caller can open it in a browser instead.
#[derive(Debug)]
pub struct NeedsYtDlp(pub String);

impl std::fmt::Display for NeedsYtDlp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "playing {} needs yt-dlp", self.0)
    }
}

impl std::error::Error for NeedsYtDlp {}

/// Video sites whose pages aren't streams themselves.
fn is_web_page(url: &str) -> bool {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default()
        .to_ascii_lowercase();
    ["youtube.com", "youtu.be", "vimeo.com", "dailymotion.com"]
        .iter()
        .any(|site| host == *site || host.ends_with(&format!(".{site}")))
}

/// A URL mpv can play: direct links as they are, video-site pages through
/// yt-dlp (blocking; run off the main thread).
pub fn resolve(url: &str) -> Result<String> {
    if !is_web_page(url) {
        return Ok(url.to_string());
    }
    let output = match Command::new("yt-dlp")
        .args(["--no-playlist", "-f", "best[height<=1080]/best", "-g", url])
        .output()
    {
        Ok(output) => output,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(NeedsYtDlp(url.to_string()).into());
        }
        Err(e) => return Err(e).context("couldn't run yt-dlp"),
    };
    if !output.status.success() {
        bail!(
            "yt-dlp couldn't read {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::to_string)
        .context("yt-dlp returned no stream")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_video_site_pages_need_yt_dlp() {
        assert!(is_web_page("https://www.youtube.com/watch?v=abc"));
        assert!(is_web_page("https://youtu.be/abc"));
        assert!(is_web_page("https://m.youtube.com/watch?v=abc"));
        assert!(!is_web_page("https://cdn.example.com/trailer.mp4"));
        assert!(!is_web_page("https://notyoutube.com/x"));
        assert_eq!(
            resolve("https://cdn.example.com/t.mp4").unwrap(),
            "https://cdn.example.com/t.mp4"
        );
    }
}
