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

/// What mpv opens: a stream, plus a separate audio stream when the site
/// serves them apart (YouTube no longer offers combined ones in HD), and
/// captions when wanted and available.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub url: String,
    pub audio: Option<String>,
    pub subtitle: Option<String>,
}

/// How to fetch a web video.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    /// Tallest picture to fetch; `None` takes the best there is.
    pub max_height: Option<u32>,
    /// Captions language (ISO 639-2, as in the settings), if wanted.
    pub captions: Option<String>,
}

impl Options {
    pub fn from_settings(settings: &crate::config::Settings) -> Self {
        Options {
            max_height: settings.playback.trailer_quality.max_height(),
            captions: settings
                .playback
                .trailer_captions
                .then(|| settings.subtitles.preferred_language.clone()),
        }
    }
}

/// A stream mpv can play: direct links as they are, video-site pages
/// through yt-dlp (blocking; run off the main thread).
pub fn resolve(url: &str, options: &Options) -> Result<Stream> {
    if !is_web_page(url) {
        return Ok(Stream {
            url: url.to_string(),
            audio: None,
            subtitle: None,
        });
    }
    let output = match Command::new("yt-dlp")
        .args([
            "--no-playlist",
            "-f",
            &format_selector(options.max_height),
            "-J",
            url,
        ])
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
    let info: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("yt-dlp returned unreadable data")?;
    let captions = options.captions.as_deref().map(two_letter);
    parse_info(&info, captions).context("yt-dlp returned no stream")
}

/// Best video (up to `max_height`) with the best audio; a combined stream
/// only as the fallback.
fn format_selector(max_height: Option<u32>) -> String {
    match max_height {
        Some(h) => format!("bv*[height<={h}]+ba/b[height<={h}]/bv*+ba/b"),
        None => "bv*+ba/b".to_string(),
    }
}

fn parse_info(info: &serde_json::Value, captions: Option<&str>) -> Option<Stream> {
    let url_of = |v: &serde_json::Value| v.get("url")?.as_str().map(str::to_string);
    let (url, audio) = match info.get("requested_formats").and_then(|f| f.as_array()) {
        Some(formats) if !formats.is_empty() => {
            (url_of(&formats[0])?, formats.get(1).and_then(url_of))
        }
        _ => (url_of(info)?, None),
    };
    let subtitle = captions.and_then(|language| {
        // The uploader's own captions first, then YouTube's automatic ones.
        ["subtitles", "automatic_captions"]
            .iter()
            .find_map(|kind| caption_url(info.get(kind)?, language))
    });
    Some(Stream {
        url,
        audio,
        subtitle,
    })
}

/// A WebVTT caption URL for `language` ("en"), also matching regional or
/// original-language variants ("en-US", "en-orig").
fn caption_url(tracks: &serde_json::Value, language: &str) -> Option<String> {
    let tracks = tracks.as_object()?;
    let mut keys: Vec<&String> = tracks
        .keys()
        .filter(|key| *key == language || key.starts_with(&format!("{language}-")))
        .collect();
    // Exact first, then the original-language track, then regional ones.
    keys.sort_by_key(|key| (key.as_str() != language, !key.ends_with("-orig")));
    keys.into_iter().find_map(|key| {
        tracks[key.as_str()]
            .as_array()?
            .iter()
            .find(|t| t.get("ext").and_then(|e| e.as_str()) == Some("vtt"))?
            .get("url")?
            .as_str()
            .map(str::to_string)
    })
}

/// ISO 639-2 (the settings' codes) to the two-letter codes sites use.
fn two_letter(code: &str) -> &str {
    match code {
        "eng" => "en",
        "jpn" => "ja",
        "zho" | "chi" => "zh",
        "kor" => "ko",
        "ind" => "id",
        "msa" | "may" => "ms",
        "tha" => "th",
        "vie" => "vi",
        "spa" => "es",
        "por" => "pt",
        "fra" | "fre" => "fr",
        "deu" | "ger" => "de",
        "ita" => "it",
        "rus" => "ru",
        "ara" => "ar",
        "nld" | "dut" => "nl",
        other => other,
    }
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
            resolve("https://cdn.example.com/t.mp4", &Options::default())
                .unwrap()
                .url,
            "https://cdn.example.com/t.mp4"
        );
    }

    #[test]
    fn split_streams_carry_their_audio_and_captions() {
        let info = serde_json::json!({
            "requested_formats": [{"url": "https://v/video"}, {"url": "https://v/audio"}],
            "subtitles": {"es": [{"ext": "vtt", "url": "https://v/es.vtt"}]},
            "automatic_captions": {
                "en": [{"ext": "json3", "url": "https://v/en.json"}, {"ext": "vtt", "url": "https://v/en.vtt"}],
                "en-orig": [{"ext": "vtt", "url": "https://v/en-orig.vtt"}]
            }
        });
        let stream = parse_info(&info, Some("en")).unwrap();
        assert_eq!(stream.url, "https://v/video");
        assert_eq!(stream.audio.as_deref(), Some("https://v/audio"));
        assert_eq!(stream.subtitle.as_deref(), Some("https://v/en.vtt"));
        assert_eq!(
            parse_info(&info, Some("es")).unwrap().subtitle.as_deref(),
            Some("https://v/es.vtt")
        );
        assert_eq!(parse_info(&info, None).unwrap().subtitle, None);
        assert_eq!(parse_info(&info, Some("ja")).unwrap().subtitle, None);
    }

    #[test]
    fn combined_streams_and_format_choice() {
        let info = serde_json::json!({"url": "https://v/combined"});
        let stream = parse_info(&info, None).unwrap();
        assert_eq!(
            (stream.url.as_str(), stream.audio),
            ("https://v/combined", None)
        );
        assert_eq!(format_selector(None), "bv*+ba/b");
        assert!(format_selector(Some(1080)).starts_with("bv*[height<=1080]+ba"));
        assert_eq!(two_letter("eng"), "en");
    }
}
