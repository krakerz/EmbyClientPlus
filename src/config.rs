use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    #[serde(default)]
    pub server: ServerSettings,
    #[serde(default)]
    pub playback: PlaybackSettings,
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub subtitles: SubtitleSettings,
    #[serde(default)]
    pub frame_gen: FrameGenSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerSettings {
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackSettings {
    #[serde(default)]
    pub mode: PlaybackMode,
    /// 0 means unlimited.
    #[serde(default)]
    pub bitrate_cap_mbps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    #[default]
    DirectPlayPreferred,
    CapBitrate,
    Ask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioSettings {
    #[serde(default = "default_language")]
    pub preferred_language: String,
    #[serde(default)]
    pub passthrough: AudioPassthrough,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            preferred_language: default_language(),
            passthrough: AudioPassthrough::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AudioPassthrough {
    #[default]
    Auto,
    Always,
    Downmix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleSettings {
    #[serde(default = "default_language")]
    pub preferred_language: String,
    #[serde(default = "default_true")]
    pub prefer_text_over_burnin: bool,
}

impl Default for SubtitleSettings {
    fn default() -> Self {
        Self {
            preferred_language: default_language(),
            prefer_text_over_burnin: true,
        }
    }
}

/// Interpolation settings themselves (multiplier, quality) live in SVP
/// Manager, which attaches over mpv's IPC socket — we only decide whether
/// to expose the socket at all.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FrameGenSettings {
    #[serde(default)]
    pub default_backend: FrameGenBackend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FrameGenBackend {
    /// `lsfg_vk` was a backend before 0.2.0; old configs map it to off.
    #[default]
    #[serde(alias = "lsfg_vk")]
    Off,
    Svp,
}

fn default_language() -> String {
    "eng".to_string()
}

fn default_true() -> bool {
    true
}

/// Directory holding config.toml, the SQLite override db, and any other
/// per-install state (not XDG data/cache — kept together deliberately
/// since this app has no large asset cache to separate out).
pub fn config_dir() -> Result<PathBuf> {
    ProjectDirs::from("com", "krakerz", "embyclientplus")
        .map(|dirs| dirs.config_dir().to_path_buf())
        .context("could not determine config directory (no valid $HOME?)")
}

fn config_file_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

impl Settings {
    /// Loads settings from disk, falling back to defaults if the file
    /// doesn't exist yet (first run).
    pub fn load() -> Result<Settings> {
        let path = config_file_path()?;
        if !path.exists() {
            return Ok(Settings::default());
        }
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_file_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let contents = toml::to_string_pretty(self).context("failed to serialize settings")?;
        std::fs::write(&path, contents)
            .with_context(|| format!("failed to write {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_toml() {
        let settings = Settings::default();
        let serialized = toml::to_string_pretty(&settings).unwrap();
        let parsed: Settings = toml::from_str(&serialized).unwrap();
        assert_eq!(parsed.playback.mode, PlaybackMode::DirectPlayPreferred);
        assert_eq!(parsed.frame_gen.default_backend, FrameGenBackend::Off);
    }

    #[test]
    fn legacy_lsfg_config_still_parses_as_off() {
        let legacy = r#"
            [frame_gen]
            default_backend = "lsfg_vk"

            [frame_gen.lsfg_vk]
            multiplier = 3
            flow_scale = 0.8
        "#;
        let parsed: Settings = toml::from_str(legacy).unwrap();
        assert_eq!(parsed.frame_gen.default_backend, FrameGenBackend::Off);
    }

    #[test]
    fn partial_toml_fills_in_defaults() {
        let partial = r#"
            [server]
            url = "http://192.168.1.3:8096"

            [frame_gen]
            default_backend = "svp"
        "#;
        let parsed: Settings = toml::from_str(partial).unwrap();
        assert_eq!(parsed.server.url, "http://192.168.1.3:8096");
        assert_eq!(parsed.frame_gen.default_backend, FrameGenBackend::Svp);
        // Untouched sections still get their defaults.
        assert_eq!(parsed.audio.preferred_language, "eng");
        assert_eq!(parsed.playback.mode, PlaybackMode::DirectPlayPreferred);
    }
}
