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
    #[serde(default)]
    pub controller: ControllerSettings,
    #[serde(default)]
    pub home: HomeSettings,
    #[serde(default)]
    pub window: WindowSettings,
    #[serde(default)]
    pub updates: UpdateSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSettings {
    #[serde(default = "default_true")]
    pub check_on_start: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_on_start: true,
        }
    }
}

/// Start-up window size, applied before any fullscreen request so a cold
/// launch in gamescope never falls back to 800×600.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowSettings {
    #[serde(default = "default_width")]
    pub width: i32,
    #[serde(default = "default_height")]
    pub height: i32,
    #[serde(default)]
    pub fullscreen: FullscreenMode,
    #[serde(default)]
    pub theme: Theme,
}

/// Colour scheme. Dark by default: SteamOS reports a light preference, so
/// following the system would make Game Mode white.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    #[default]
    Dark,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: default_width(),
            height: default_height(),
            fullscreen: FullscreenMode::default(),
            theme: Theme::default(),
        }
    }
}

impl WindowSettings {
    pub fn start_fullscreen(&self, in_gamescope: bool) -> bool {
        match self.fullscreen {
            FullscreenMode::Auto => in_gamescope,
            FullscreenMode::Always => true,
            FullscreenMode::Never => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FullscreenMode {
    /// Fullscreen in gamescope (Steam Game Mode), windowed elsewhere.
    #[default]
    Auto,
    Always,
    Never,
}

/// The Steam Deck's screen.
fn default_width() -> i32 {
    1280
}

fn default_height() -> i32 {
    800
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HomeSettings {
    /// Art on Continue Watching / Next Up cards.
    #[serde(default)]
    pub episode_art: EpisodeArt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeArt {
    /// The episode's own still.
    #[default]
    Episode,
    /// The series' art, so a still can't spoil what happens.
    Series,
}

/// Gamepad mapping: action name → button names, only for actions changed
/// from their defaults (see `controller::bindings`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ControllerSettings {
    #[serde(default)]
    pub bindings: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerSettings {
    #[serde(default)]
    pub url: String,
    /// Logged-in user; the token itself lives in the keyring (`auth.rs`).
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub username: String,
    /// Stable per-install id so Emby sees one device across launches.
    #[serde(default)]
    pub device_id: String,
}

impl ServerSettings {
    /// Generates the device id on first use; returns whether it was new
    /// (and so needs saving).
    pub fn ensure_device_id(&mut self) -> bool {
        if !self.device_id.is_empty() {
            return false;
        }
        self.device_id = uuid::Uuid::new_v4().to_string();
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackSettings {
    #[serde(default)]
    pub mode: PlaybackMode,
    /// 0 means unlimited.
    #[serde(default)]
    pub bitrate_cap_mbps: u32,
    /// Load ~/.config/mpv/mpv.conf (minus options the app manages).
    #[serde(default)]
    pub use_mpv_conf: bool,
    /// What fills the black bars: "off", "blur" or "glow".
    #[serde(default = "default_bar_fill")]
    pub bar_fill: String,
    /// Web trailers (YouTube): the tallest picture to fetch.
    #[serde(default)]
    pub trailer_quality: TrailerQuality,
    /// Web trailers: show captions in the preferred subtitle language.
    #[serde(default = "default_true")]
    pub trailer_captions: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrailerQuality {
    #[default]
    Best,
    #[serde(rename = "2160p")]
    P2160,
    #[serde(rename = "1440p")]
    P1440,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "720p")]
    P720,
}

impl TrailerQuality {
    pub const ALL: [TrailerQuality; 5] = [
        TrailerQuality::Best,
        TrailerQuality::P2160,
        TrailerQuality::P1440,
        TrailerQuality::P1080,
        TrailerQuality::P720,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TrailerQuality::Best => "Best available",
            TrailerQuality::P2160 => "Up to 4K",
            TrailerQuality::P1440 => "Up to 1440p",
            TrailerQuality::P1080 => "Up to 1080p",
            TrailerQuality::P720 => "Up to 720p",
        }
    }

    pub fn max_height(self) -> Option<u32> {
        match self {
            TrailerQuality::Best => None,
            TrailerQuality::P2160 => Some(2160),
            TrailerQuality::P1440 => Some(1440),
            TrailerQuality::P1080 => Some(1080),
            TrailerQuality::P720 => Some(720),
        }
    }
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            mode: PlaybackMode::default(),
            bitrate_cap_mbps: 0,
            use_mpv_conf: false,
            bar_fill: default_bar_fill(),
            trailer_quality: TrailerQuality::default(),
            trailer_captions: true,
        }
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameGenSettings {
    #[serde(default)]
    pub default_backend: FrameGenBackend,
    /// SVP's install folder; empty means `~/SVP4`.
    #[serde(default)]
    pub svp_dir: String,
    /// IPC socket SVP Manager connects to (SVP's own mpv.conf uses this).
    #[serde(default = "default_svp_socket")]
    pub svp_socket: String,
    /// Start SVP Manager when SVP is wanted and it isn't running. Unset
    /// means "only in gamescope", where nothing else would start it.
    #[serde(default)]
    pub auto_start_svp: Option<bool>,
    /// mpv's own frame blending when SVP isn't interpolating.
    #[serde(default)]
    pub smooth_without_svp: bool,
}

pub const DEFAULT_SVP_SOCKET: &str = "/tmp/mpvsocket";

fn default_svp_socket() -> String {
    DEFAULT_SVP_SOCKET.to_string()
}

impl Default for FrameGenSettings {
    fn default() -> Self {
        Self {
            default_backend: FrameGenBackend::default(),
            svp_dir: String::new(),
            svp_socket: default_svp_socket(),
            auto_start_svp: None,
            smooth_without_svp: false,
        }
    }
}

impl FrameGenSettings {
    pub fn auto_start_svp(&self) -> bool {
        self.auto_start_svp
            .unwrap_or_else(crate::gamescope::detected)
    }

    /// The configured socket, falling back to the default when blank.
    pub fn socket(&self) -> &str {
        match self.svp_socket.trim() {
            "" => DEFAULT_SVP_SOCKET,
            socket => socket,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FrameGenBackend {
    /// `lsfg_vk` was a backend before 0.2.0; old configs map it to off.
    #[serde(alias = "lsfg_vk")]
    Off,
    /// The default: SVP attaches whenever SVP Manager is running.
    #[default]
    Svp,
}

fn default_bar_fill() -> String {
    "off".to_string()
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

    /// Re-reads the file, applies `change`, and writes it back, so edits
    /// from different places (login, preferences) never clobber each other.
    pub fn update(change: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut settings = Settings::load()?;
        change(&mut settings);
        settings.save()?;
        Ok(settings)
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
        assert_eq!(parsed.frame_gen.default_backend, FrameGenBackend::Svp);
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
    fn window_defaults_and_fullscreen_modes() {
        let window = Settings::default().window;
        assert_eq!((window.width, window.height), (1280, 800));
        assert!(window.start_fullscreen(true));
        assert!(!window.start_fullscreen(false));
        let parsed: Settings = toml::from_str("[window]\nfullscreen = \"never\"\n").unwrap();
        assert!(!parsed.window.start_fullscreen(true));
        assert_eq!(parsed.window.width, 1280);
    }

    #[test]
    fn device_id_is_generated_once() {
        let mut server = ServerSettings::default();
        assert!(server.ensure_device_id());
        let id = server.device_id.clone();
        assert!(!server.ensure_device_id());
        assert_eq!(server.device_id, id);
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
        assert!(parsed.server.user_id.is_empty());
        assert_eq!(parsed.frame_gen.default_backend, FrameGenBackend::Svp);
        assert_eq!(parsed.frame_gen.socket(), "/tmp/mpvsocket");
        // Untouched sections still get their defaults.
        assert_eq!(parsed.audio.preferred_language, "eng");
        assert_eq!(parsed.playback.mode, PlaybackMode::DirectPlayPreferred);
    }
}
