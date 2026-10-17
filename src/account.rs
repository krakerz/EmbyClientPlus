//! Whose settings apply. Signed in to Emby, the account's own (Emby's
//! `UserConfiguration`, shared with Emby's apps, edited on the Profile
//! page); in player-only mode, this app's config.toml, so playing files
//! never needs a connection.
//!
//! The account's configuration is kept whole (a JSON object) so saving it
//! back sends every field, including ones this app doesn't use. A copy
//! lives in the config folder for signed-in use without a connection
//! (downloads).

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::config::{IntroSkip, Settings, SubtitleMode};

struct Account {
    user_id: String,
    /// The user as Emby sent it (`UserDto`); its `Configuration` is ours to
    /// edit.
    user: Value,
}

static ACCOUNT: Mutex<Option<Account>> = Mutex::new(None);

/// The playback choices in effect, from whichever source applies.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackPrefs {
    /// Empty: no preference.
    pub audio_language: String,
    /// Keep the file's default audio track, whatever its language.
    pub play_default_audio: bool,
    /// Empty: no preference.
    pub subtitle_language: String,
    pub subtitle_mode: SubtitleMode,
    pub remember_audio: bool,
    pub remember_subtitles: bool,
    pub autoplay_next: bool,
    pub resume_rewind_seconds: u32,
    pub intro_skip: IntroSkip,
}

impl PlaybackPrefs {
    fn local(settings: &Settings) -> Self {
        PlaybackPrefs {
            audio_language: settings.audio.preferred_language.clone(),
            play_default_audio: settings.audio.play_default_track,
            subtitle_language: settings.subtitles.preferred_language.clone(),
            subtitle_mode: settings.subtitles.mode,
            remember_audio: true,
            remember_subtitles: true,
            autoplay_next: settings.playback.autoplay_next,
            resume_rewind_seconds: settings.playback.resume_rewind_seconds,
            intro_skip: settings.playback.intro_skip,
        }
    }

    fn from_configuration(config: &Map<String, Value>) -> Self {
        let text = |key: &str| config.get(key).and_then(Value::as_str).unwrap_or_default();
        let flag =
            |key: &str, default: bool| config.get(key).and_then(Value::as_bool).unwrap_or(default);
        PlaybackPrefs {
            audio_language: text("AudioLanguagePreference").to_string(),
            play_default_audio: flag("PlayDefaultAudioTrack", true),
            subtitle_language: text("SubtitleLanguagePreference").to_string(),
            subtitle_mode: serde_json::from_value(Value::from(text("SubtitleMode")))
                .unwrap_or(SubtitleMode::Default),
            remember_audio: flag("RememberAudioSelections", true),
            remember_subtitles: flag("RememberSubtitleSelections", true),
            autoplay_next: flag("EnableNextEpisodeAutoPlay", true),
            resume_rewind_seconds: config
                .get("ResumeRewindSeconds")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32,
            intro_skip: serde_json::from_value(Value::from(text("IntroSkipMode")))
                .unwrap_or_default(),
        }
    }
}

/// The playback choices in effect now.
pub fn playback() -> PlaybackPrefs {
    let account = ACCOUNT.lock().unwrap_or_else(|e| e.into_inner());
    match account
        .as_ref()
        .and_then(|a| a.user.get("Configuration")?.as_object().cloned())
    {
        Some(config) => PlaybackPrefs::from_configuration(&config),
        None => PlaybackPrefs::local(&Settings::load().unwrap_or_default()),
    }
}

/// Whether suggestions leave out watched titles (Emby's
/// `HidePlayedInSuggestions`); without an account, they do.
pub fn hide_played_in_suggestions() -> bool {
    user()
        .and_then(|user| {
            user.get("Configuration")?
                .get("HidePlayedInSuggestions")?
                .as_bool()
        })
        .unwrap_or(true)
}

/// The account's user object, if signed in.
pub fn user() -> Option<Value> {
    ACCOUNT
        .lock()
        .ok()?
        .as_ref()
        .map(|account| account.user.clone())
}

/// Signed in as `user_id`: its saved copy applies until the server's
/// arrives (`refresh`), so settings work without a connection too.
pub fn sign_in(user_id: &str) {
    let user = read_copy(user_id).unwrap_or_else(|| serde_json::json!({ "Configuration": {} }));
    *ACCOUNT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Account {
        user_id: user_id.to_string(),
        user,
    });
}

/// Back to this app's own settings.
pub fn sign_out() {
    ACCOUNT.lock().unwrap_or_else(|e| e.into_inner()).take();
}

/// Takes the server's current copy of the user.
pub fn refreshed(user_id: &str, user: Value) {
    {
        let mut account = ACCOUNT.lock().unwrap_or_else(|e| e.into_inner());
        match account.as_mut() {
            Some(account) if account.user_id == user_id => account.user = user.clone(),
            _ => return,
        }
    }
    write_copy(user_id, &user);
}

/// Changes the account's configuration here (saved copy included) and
/// returns the whole new configuration, for sending to the server.
pub fn change(edit: impl FnOnce(&mut Map<String, Value>)) -> Option<(String, Value)> {
    let (user_id, user) = {
        let mut account = ACCOUNT.lock().unwrap_or_else(|e| e.into_inner());
        let account = account.as_mut()?;
        if !account
            .user
            .get("Configuration")
            .is_some_and(Value::is_object)
        {
            account.user["Configuration"] = Value::Object(Map::new());
        }
        edit(account.user["Configuration"].as_object_mut()?);
        (account.user_id.clone(), account.user.clone())
    };
    write_copy(&user_id, &user);
    Some((user_id, user["Configuration"].clone()))
}

fn copy_path(user_id: &str) -> Result<PathBuf> {
    let safe: String = user_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    Ok(crate::config::config_dir()?.join(format!("account-{safe}.json")))
}

fn read_copy(user_id: &str) -> Option<Value> {
    let text = std::fs::read_to_string(copy_path(user_id).ok()?).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_copy(user_id: &str, user: &Value) {
    let result = (|| -> Result<()> {
        let path = copy_path(user_id)?;
        let text = serde_json::to_string_pretty(user)?;
        crate::config::write_atomic(&path, text.as_bytes())
            .with_context(|| format!("writing {}", path.display()))
    })();
    if let Err(e) = result {
        tracing::warn!("couldn't save the account's settings: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_emby_configuration() {
        let config = serde_json::json!({
            "AudioLanguagePreference": "",
            "SubtitleLanguagePreference": "en",
            "SubtitleMode": "Smart",
            "PlayDefaultAudioTrack": true,
            "EnableNextEpisodeAutoPlay": false,
            "ResumeRewindSeconds": 5,
            "IntroSkipMode": "AutoSkip",
            "RememberAudioSelections": false
        });
        let prefs = PlaybackPrefs::from_configuration(config.as_object().unwrap());
        assert_eq!(prefs.subtitle_language, "en");
        assert_eq!(prefs.subtitle_mode, SubtitleMode::Smart);
        assert!(prefs.play_default_audio && !prefs.autoplay_next && !prefs.remember_audio);
        assert!(prefs.remember_subtitles);
        assert_eq!(prefs.resume_rewind_seconds, 5);
        assert_eq!(prefs.intro_skip, IntroSkip::AutoSkip);
        // Missing or unknown values fall back to Emby's defaults.
        let prefs = PlaybackPrefs::from_configuration(&Map::new());
        assert_eq!(prefs.subtitle_mode, SubtitleMode::Default);
        assert_eq!(prefs.intro_skip, IntroSkip::ShowButton);
    }
}
