//! Per-library audio and subtitle defaults. They are stored as
//! `title_overrides` rows of type `Library`, keyed by the library's id, and
//! sit between a title's own choice and the Emby profile.

use std::sync::{Mutex, PoisonError};

use crate::db::{Db, ItemType, TitleOverride};
use crate::emby::models::BaseItem;

/// A library that can hold playable titles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Library {
    pub id: String,
    pub name: String,
}

/// Global, not thread-local: Home fills it from a tokio worker, Preferences reads it on the GTK thread.
static LIBRARIES: Mutex<Vec<Library>> = Mutex::new(Vec::new());

/// Keeps the video libraries among the user's views (Home calls this).
pub fn remember_libraries(views: &[BaseItem]) {
    let found = views
        .iter()
        .filter(|v| {
            matches!(
                v.collection_type.as_deref(),
                None | Some("movies" | "tvshows" | "mixed" | "homevideos")
            )
        })
        .map(|v| Library {
            id: v.id.clone(),
            name: v.name.clone(),
        })
        .collect();
    *LIBRARIES.lock().unwrap_or_else(PoisonError::into_inner) = found;
}

/// The libraries seen on Home so far; empty before it has loaded.
pub fn libraries() -> Vec<Library> {
    LIBRARIES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The defaults saved for a library, if any.
pub fn defaults(library_id: &str) -> Option<TitleOverride> {
    crate::playback::remembered_for(library_id).filter(|o| o.item_type == ItemType::Library)
}

/// Saves a library's defaults. `audio` is a language code (None: the Emby
/// profile); `subtitle` a code, `tracks::SUBTITLES_OFF` or None. Both None
/// forgets the library.
pub fn set(library_id: &str, audio: Option<String>, subtitle: Option<String>) {
    if audio.is_none() && subtitle.is_none() {
        let result = Db::open_default()
            .and_then(|db| db.delete_override(&crate::account::scope(), library_id));
        if let Err(e) = result {
            tracing::warn!("could not clear the library languages: {e:#}");
        }
        return;
    }
    crate::playback::remember_for(library_id, ItemType::Library, |o| {
        o.audio_language = audio;
        o.subtitle_language = subtitle;
        o.subtitle_forced_only = Some(false);
    });
}

/// The language choices to start a title with.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TrackPrefs {
    pub audio: Option<String>,
    pub subtitle: Option<String>,
    pub forced_only: bool,
}

/// Per field, the title's own choice first, then the library's.
pub fn merge(title: Option<&TitleOverride>, library: Option<&TitleOverride>) -> TrackPrefs {
    let audio = title
        .and_then(|o| o.audio_language.clone())
        .or_else(|| library.and_then(|o| o.audio_language.clone()));
    let (subtitle, source) = match title.and_then(|o| o.subtitle_language.clone()) {
        Some(code) => (Some(code), title),
        None => (library.and_then(|o| o.subtitle_language.clone()), library),
    };
    let forced_only = source.and_then(|o| o.subtitle_forced_only).unwrap_or(false);
    TrackPrefs {
        audio,
        subtitle,
        forced_only,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(audio: Option<&str>, subtitle: Option<&str>, forced: Option<bool>) -> TitleOverride {
        TitleOverride {
            emby_item_id: "x".into(),
            item_type: ItemType::Series,
            audio_language: audio.map(Into::into),
            subtitle_language: subtitle.map(Into::into),
            subtitle_forced_only: forced,
            frame_gen_backend: None,
            frame_gen_multiplier: None,
            aspect_mode: None,
            zoom: None,
            shaders: None,
            volume: None,
            auto_skip: None,
            download_quality: None,
        }
    }

    #[test]
    fn title_beats_library_per_field() {
        let title = row(Some("jpn"), None, None);
        let library = row(Some("eng"), Some("eng"), Some(false));
        let got = merge(Some(&title), Some(&library));
        assert_eq!(got.audio.as_deref(), Some("jpn"));
        assert_eq!(got.subtitle.as_deref(), Some("eng"));
    }

    #[test]
    fn library_fills_a_title_that_only_has_volume() {
        let title = row(None, None, None);
        let library = row(Some("eng"), Some("off"), Some(false));
        let got = merge(Some(&title), Some(&library));
        assert_eq!(got.audio.as_deref(), Some("eng"));
        assert_eq!(got.subtitle.as_deref(), Some("off"));
    }

    #[test]
    fn nothing_gives_nothing() {
        assert_eq!(merge(None, None), TrackPrefs::default());
    }

    #[test]
    fn forced_only_follows_the_subtitle_source() {
        let title = row(None, Some("eng"), Some(true));
        let library = row(None, Some("fra"), Some(false));
        assert!(merge(Some(&title), Some(&library)).forced_only);
        let title = row(Some("jpn"), None, Some(true));
        assert!(!merge(Some(&title), Some(&library)).forced_only);
    }
}
