//! Audio/subtitle track choice: matching remembered and preferred
//! languages against what mpv actually loaded, and labelling tracks.

use crate::emby::models::MediaStream;
use crate::player::{Track, TrackKind};

/// Stored in `TitleOverride::subtitle_language` when subtitles were turned off.
pub const SUBTITLES_OFF: &str = "off";

/// Normalizes language codes to ISO 639-2 (3 letters, lowercase), so
/// mpv's and Emby's `en`/`eng`/`ENG` all compare equal.
pub fn normalize_language(language: &str) -> String {
    let lower = language.trim().to_ascii_lowercase();
    let mapped = match lower.as_str() {
        "en" => "eng",
        "ja" | "jp" => "jpn",
        "zh" | "chi" => "zho",
        "ko" => "kor",
        "fr" | "fre" => "fra",
        "de" | "ger" => "deu",
        "es" => "spa",
        "it" => "ita",
        "pt" => "por",
        "ru" => "rus",
        "id" => "ind",
        "ms" | "may" => "msa",
        "th" => "tha",
        "vi" => "vie",
        "ar" => "ara",
        "nl" | "dut" => "nld",
        other => other,
    };
    mapped.to_string()
}

fn same_language(a: Option<&str>, b: &str) -> bool {
    a.is_some_and(|a| normalize_language(a) == normalize_language(b))
}

/// The audio track to start with: the language remembered for this title,
/// else the configured preference; `None` keeps mpv's own pick.
pub fn initial_audio(tracks: &[Track], remembered: Option<&str>, preferred: &str) -> Option<i64> {
    let audio: Vec<&Track> = tracks
        .iter()
        .filter(|t| t.kind == Some(TrackKind::Audio))
        .collect();
    [remembered, Some(preferred)]
        .into_iter()
        .flatten()
        .find_map(|lang| {
            audio
                .iter()
                .find(|t| same_language(t.lang.as_deref(), lang))
                .map(|t| t.id)
        })
}

/// What to do with subtitles at start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleStart {
    /// Leave mpv's default (usually the container's default track).
    Keep,
    Off,
    Select(i64),
}

/// Remembered choice wins ("off" included); otherwise the preferred
/// language if present. With `forced_only`, only forced tracks qualify.
pub fn initial_subtitle(
    tracks: &[Track],
    remembered: Option<&str>,
    forced_only: bool,
    preferred: &str,
) -> SubtitleStart {
    if remembered == Some(SUBTITLES_OFF) {
        return SubtitleStart::Off;
    }
    let subtitles: Vec<&Track> = tracks
        .iter()
        .filter(|t| t.kind == Some(TrackKind::Subtitle))
        .filter(|t| !forced_only || t.forced)
        .collect();
    let pick = |lang: &str| {
        let matching: Vec<&&Track> = subtitles
            .iter()
            .filter(|t| same_language(t.lang.as_deref(), lang))
            .collect();
        // Full subtitles over signs/songs-only tracks unless forced is wanted.
        matching
            .iter()
            .find(|t| t.forced == forced_only)
            .or(matching.first())
            .map(|t| t.id)
    };
    match remembered.and_then(pick).or_else(|| pick(preferred)) {
        Some(id) => SubtitleStart::Select(id),
        None if forced_only => SubtitleStart::Off,
        None => SubtitleStart::Keep,
    }
}

/// The Emby `MediaStream` an mpv track came from: embedded tracks by
/// container index, external ones by the URL they were loaded from.
pub fn emby_stream<'a>(
    track: &Track,
    streams: &'a [MediaStream],
    external_urls: &[(i32, String)],
) -> Option<&'a MediaStream> {
    let index = if track.external {
        let file = track.external_filename.as_deref()?;
        external_urls
            .iter()
            .find(|(_, url)| url == file)
            .map(|(index, _)| *index)?
    } else {
        i32::try_from(track.ff_index?).ok()?
    };
    streams.iter().find(|s| s.index == index)
}

/// Menu label: Emby's display title when we can map the track, else
/// built from mpv's metadata.
pub fn track_label(track: &Track, stream: Option<&MediaStream>) -> String {
    let mut label = stream
        .and_then(|s| s.display_title.clone())
        .or_else(|| {
            let parts: Vec<String> = [
                track.title.clone(),
                track.lang.as_deref().map(language_name),
                track.codec.as_deref().map(str::to_uppercase),
            ]
            .into_iter()
            .flatten()
            .collect();
            (!parts.is_empty()).then(|| parts.join(" · "))
        })
        .unwrap_or_else(|| format!("Track {}", track.id));
    if track.forced && !label.to_lowercase().contains("forced") {
        label.push_str(" (Forced)");
    }
    label
}

/// Human name for common codes; the code itself otherwise.
fn language_name(code: &str) -> String {
    let name = match normalize_language(code).as_str() {
        "eng" => "English",
        "jpn" => "Japanese",
        "zho" => "Chinese",
        "kor" => "Korean",
        "fra" => "French",
        "deu" => "German",
        "spa" => "Spanish",
        "ita" => "Italian",
        "por" => "Portuguese",
        "rus" => "Russian",
        "ind" => "Indonesian",
        "msa" => "Malay",
        "tha" => "Thai",
        "vie" => "Vietnamese",
        "ara" => "Arabic",
        "nld" => "Dutch",
        "und" => "Unknown",
        _ => return code.to_string(),
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64, kind: TrackKind, lang: &str, forced: bool) -> Track {
        Track {
            id,
            kind: Some(kind),
            lang: Some(lang.into()),
            forced,
            ff_index: Some(id),
            ..Default::default()
        }
    }

    fn sample() -> Vec<Track> {
        vec![
            track(1, TrackKind::Audio, "jpn", false),
            track(2, TrackKind::Audio, "eng", false),
            track(1, TrackKind::Subtitle, "eng", true),
            track(2, TrackKind::Subtitle, "en", false),
            track(3, TrackKind::Subtitle, "ind", false),
        ]
    }

    #[test]
    fn languages_normalize_across_code_styles() {
        assert_eq!(normalize_language("EN"), "eng");
        assert_eq!(normalize_language("jpn"), "jpn");
        assert_eq!(normalize_language("ger"), "deu");
    }

    #[test]
    fn remembered_audio_beats_preference() {
        assert_eq!(initial_audio(&sample(), Some("ja"), "eng"), Some(1));
        assert_eq!(initial_audio(&sample(), None, "eng"), Some(2));
        assert_eq!(initial_audio(&sample(), None, "fra"), None);
    }

    #[test]
    fn subtitles_prefer_full_tracks_over_forced() {
        assert_eq!(
            initial_subtitle(&sample(), None, false, "eng"),
            SubtitleStart::Select(2)
        );
        assert_eq!(
            initial_subtitle(&sample(), None, true, "eng"),
            SubtitleStart::Select(1)
        );
    }

    #[test]
    fn remembered_subtitle_choice_including_off() {
        assert_eq!(
            initial_subtitle(&sample(), Some("ind"), false, "eng"),
            SubtitleStart::Select(3)
        );
        assert_eq!(
            initial_subtitle(&sample(), Some(SUBTITLES_OFF), false, "eng"),
            SubtitleStart::Off
        );
        assert_eq!(
            initial_subtitle(&sample(), None, false, "kor"),
            SubtitleStart::Keep
        );
        assert_eq!(
            initial_subtitle(&sample(), None, true, "kor"),
            SubtitleStart::Off
        );
    }

    fn stream(index: i32, title: &str) -> MediaStream {
        serde_json::from_value(serde_json::json!({
            "Index": index, "Type": "Subtitle", "DisplayTitle": title
        }))
        .unwrap()
    }

    #[test]
    fn tracks_map_to_emby_streams() {
        let streams = [stream(2, "English (ASS)"), stream(5, "Indonesian (SRT)")];
        let embedded = Track {
            ff_index: Some(2),
            ..Default::default()
        };
        assert_eq!(emby_stream(&embedded, &streams, &[]).unwrap().index, 2);
        let external = Track {
            external: true,
            external_filename: Some("http://s/sub5.srt".into()),
            ..Default::default()
        };
        let urls = [(5, "http://s/sub5.srt".to_string())];
        assert_eq!(emby_stream(&external, &streams, &urls).unwrap().index, 5);
        assert_eq!(
            track_label(&external, emby_stream(&external, &streams, &urls)),
            "Indonesian (SRT)"
        );
    }

    #[test]
    fn labels_fall_back_to_mpv_metadata() {
        let mut t = track(4, TrackKind::Subtitle, "jpn", true);
        t.codec = Some("ass".into());
        assert_eq!(track_label(&t, None), "Japanese · ASS (Forced)");
    }
}
