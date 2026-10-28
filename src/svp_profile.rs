//! SVP Manager's Automatic profile in its profiles.cfg: read for
//! Preferences → SVP, and patched with the user's overrides right before
//! this app starts SVP Manager, which reads the file only at startup.

use std::fmt;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::Settings;

/// SVP's built-in Automatic profile: used when none of the user's own
/// profiles (which have conditions) matches the video.
pub const AUTOMATIC: &str = "P10000001_1001_1001_1001_100000000001";
const KEY_ON: &str = "on";
const KEY_TARGET: &str = "fi_target";
const KEY_EXACT: &str = "fi_target_exact";
const KEY_SMOOTHNESS: &str = "fi_auto_adjust";
const KEY_PRESET: &str = "fi_preset";
const KEY_MASKING: &str = "fi_masking";
const BACKUP_SUFFIX: &str = ".embyclientplus-bak";
/// SVP stores some choices as e.g. 2.0001, so numbers compare loosely.
const EPSILON: f64 = 1e-4;

/// "Target frame rate" in SVP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Screen,
    Movie2,
    Movie2Half,
    Movie3,
    Movie4,
    Movie5,
    Movie6,
    ScreenHalf,
    ScreenThird,
    /// One of [`Target::FIXED_FPS`].
    Fixed(u32),
}

impl Target {
    /// SVP's fixed frame rates.
    pub const FIXED_FPS: [u32; 12] = [45, 48, 50, 60, 72, 75, 80, 90, 120, 144, 165, 240];
    const SVP_VALUES: [(f64, Target); 9] = [
        (1.0001, Self::Screen),
        (2.0001, Self::Movie2),
        (2.5001, Self::Movie2Half),
        (3.0, Self::Movie3),
        (4.0, Self::Movie4),
        (5.0, Self::Movie5),
        (6.0, Self::Movie6),
        (0.5, Self::ScreenHalf),
        (0.3333, Self::ScreenThird),
    ];

    /// The menu: the named choices in SVP's order, then the fixed rates.
    pub fn choices() -> Vec<Target> {
        Self::SVP_VALUES
            .iter()
            .map(|(_, target)| *target)
            .chain(Self::FIXED_FPS.iter().map(|fps| Self::Fixed(*fps)))
            .collect()
    }

    pub fn label(self) -> String {
        match self {
            Self::Screen => "To screen".into(),
            Self::Movie2 => "Movie ×2".into(),
            Self::Movie2Half => "Movie ×2½".into(),
            Self::Movie3 => "Movie ×3".into(),
            Self::Movie4 => "Movie ×4".into(),
            Self::Movie5 => "Movie ×5".into(),
            Self::Movie6 => "Movie ×6".into(),
            Self::ScreenHalf => "Screen ÷2".into(),
            Self::ScreenThird => "Screen ÷3".into(),
            Self::Fixed(fps) => format!("{fps} fps"),
        }
    }

    /// The number as SVP writes it.
    fn svp_value(self) -> String {
        match self {
            Self::Fixed(fps) => fps.to_string(),
            other => Self::SVP_VALUES
                .iter()
                .find(|(_, target)| *target == other)
                .map_or(1.0001, |(value, _)| *value)
                .to_string(),
        }
    }

    fn from_svp(value: f64) -> Option<Self> {
        Self::SVP_VALUES
            .iter()
            .find(|(v, _)| (v - value).abs() < EPSILON)
            .map(|(_, target)| *target)
            .or_else(|| {
                Self::FIXED_FPS
                    .into_iter()
                    .find(|fps| (f64::from(*fps) - value).abs() < EPSILON)
                    .map(Self::Fixed)
            })
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Screen => f.write_str("screen"),
            Self::Movie2 => f.write_str("x2"),
            Self::Movie2Half => f.write_str("x2.5"),
            Self::Movie3 => f.write_str("x3"),
            Self::Movie4 => f.write_str("x4"),
            Self::Movie5 => f.write_str("x5"),
            Self::Movie6 => f.write_str("x6"),
            Self::ScreenHalf => f.write_str("screen/2"),
            Self::ScreenThird => f.write_str("screen/3"),
            Self::Fixed(fps) => write!(f, "{fps}"),
        }
    }
}

impl FromStr for Target {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        Ok(match text {
            "screen" => Self::Screen,
            "x2" => Self::Movie2,
            "x2.5" => Self::Movie2Half,
            "x3" => Self::Movie3,
            "x4" => Self::Movie4,
            "x5" => Self::Movie5,
            "x6" => Self::Movie6,
            "screen/2" => Self::ScreenHalf,
            "screen/3" => Self::ScreenThird,
            other => {
                let fps: u32 = other.parse().map_err(|_| ())?;
                if !Self::FIXED_FPS.contains(&fps) {
                    return Err(());
                }
                Self::Fixed(fps)
            }
        })
    }
}

/// SVP's performance ↔ quality slider, at the stops it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Smoothness(i32);

impl Smoothness {
    /// Left end (-50) to right end (30).
    pub const STOPS: [i32; 7] = [-50, -40, -30, -20, 0, 20, 30];

    pub fn choices() -> Vec<Smoothness> {
        Self::STOPS.into_iter().map(Self).collect()
    }

    pub fn label(self) -> String {
        match self.0 {
            -50 => "−50 (Higher performance)".into(),
            30 => "30 (Higher quality)".into(),
            stop => stop.to_string(),
        }
    }

    fn svp_value(self) -> i32 {
        self.0
    }

    fn from_svp(value: f64) -> Option<Self> {
        Self::STOPS
            .into_iter()
            .find(|stop| (f64::from(*stop) - value).abs() < EPSILON)
            .map(Self)
    }
}

impl fmt::Display for Smoothness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Smoothness {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let stop: i32 = text.parse().map_err(|_| ())?;
        Self::STOPS.contains(&stop).then_some(Self(stop)).ok_or(())
    }
}

/// SVP's "Optimized for".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Film,
    Animation,
}

impl Preset {
    pub const ALL: [Preset; 2] = [Self::Film, Self::Animation];

    pub fn label(self) -> &'static str {
        match self {
            Self::Film => "Film",
            Self::Animation => "Animation",
        }
    }

    fn svp_value(self) -> u32 {
        match self {
            Self::Film => 0,
            Self::Animation => 1,
        }
    }

    fn from_svp(value: f64) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|p| (f64::from(p.svp_value()) - value).abs() < EPSILON)
    }
}

impl fmt::Display for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Film => "film",
            Self::Animation => "animation",
        })
    }
}

impl FromStr for Preset {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        match text {
            "film" => Ok(Self::Film),
            "animation" => Ok(Self::Animation),
            _ => Err(()),
        }
    }
}

/// SVP's "Artifact masking".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Masking {
    Off,
    Low,
    Medium,
    High,
}

impl Masking {
    pub const ALL: [Masking; 4] = [Self::Off, Self::Low, Self::Medium, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    fn svp_value(self) -> u32 {
        match self {
            Self::Off => 0,
            Self::Low => 50,
            Self::Medium => 100,
            Self::High => 200,
        }
    }

    fn from_svp(value: f64) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|m| (f64::from(m.svp_value()) - value).abs() < EPSILON)
    }
}

impl fmt::Display for Masking {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        })
    }
}

impl FromStr for Masking {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        match text {
            "off" => Ok(Self::Off),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            _ => Err(()),
        }
    }
}

macro_rules! serialize_as_display {
    ($($ty:ty),*) => {$(
        impl Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
    )*};
}
serialize_as_display!(Target, Smoothness, Preset, Masking);

/// What Preferences → SVP writes into the Automatic profile. `None`: keep
/// SVP's own value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Overrides {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub target: Option<Target>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact: Option<bool>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub smoothness: Option<Smoothness>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub preset: Option<Preset>,
    #[serde(
        default,
        deserialize_with = "lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub masking: Option<Masking>,
}

impl Overrides {
    pub fn is_unset(&self) -> bool {
        *self == Self::default()
    }

    /// (key name, value text as SVP writes it) for each chosen value.
    fn wanted(&self) -> Vec<(&'static str, String)> {
        let mut wanted = Vec::new();
        if let Some(on) = self.on {
            wanted.push((KEY_ON, on.to_string()));
        }
        if let Some(target) = self.target {
            wanted.push((KEY_TARGET, target.svp_value()));
        }
        if let Some(exact) = self.exact {
            wanted.push((KEY_EXACT, exact.to_string()));
        }
        if let Some(smoothness) = self.smoothness {
            wanted.push((KEY_SMOOTHNESS, smoothness.svp_value().to_string()));
        }
        if let Some(preset) = self.preset {
            wanted.push((KEY_PRESET, preset.svp_value().to_string()));
        }
        if let Some(masking) = self.masking {
            wanted.push((KEY_MASKING, masking.svp_value().to_string()));
        }
        wanted
    }
}

/// A value this version doesn't know counts as not chosen.
fn lenient<'de, D: serde::Deserializer<'de>, T: FromStr>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    let text = Option::<String>::deserialize(deserializer)?;
    Ok(text.as_deref().and_then(|t| t.parse().ok()))
}

/// One setting as SVP saved it.
#[derive(Debug, Clone, PartialEq)]
pub enum Current<T> {
    Known(T),
    /// A value none of the choices matches, as written in the file.
    Custom(String),
    /// Not in the profile (SVP's default applies).
    Missing,
}

impl<T: Copy> Current<T> {
    pub fn known(&self) -> Option<T> {
        match self {
            Self::Known(v) => Some(*v),
            _ => None,
        }
    }

    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

/// The Automatic profile's settings, and the user's own enabled profiles.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub on: Current<bool>,
    pub target: Current<Target>,
    pub exact: Current<bool>,
    pub smoothness: Current<Smoothness>,
    pub preset: Current<Preset>,
    pub masking: Current<Masking>,
    /// Titles of the user's own enabled profiles, which SVP picks instead
    /// of Automatic for the videos their conditions match.
    pub own_profiles: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum ProfileError {
    Unreadable(String),
    NoAutomaticProfile,
    MissingKey(&'static str),
    DuplicateKey(String),
    Layout(String),
    Check,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable(e) => write!(f, "not the format SVP writes: {e}"),
            Self::NoAutomaticProfile => f.write_str("no Automatic profile"),
            Self::MissingKey(k) => {
                write!(
                    f,
                    "the Automatic profile has no {k} yet; change it once in SVP"
                )
            }
            Self::DuplicateKey(k) => write!(f, "{k} appears more than once"),
            Self::Layout(k) => write!(f, "{k} isn't on a line of its own"),
            Self::Check => f.write_str("the changed file didn't read back as expected"),
        }
    }
}

impl std::error::Error for ProfileError {}

fn key(name: &str) -> String {
    format!("{AUTOMATIC}/{name}")
}

/// Where the JSON starts (after SVP's `//` header lines), and its contents.
fn json_object(text: &str) -> Result<(usize, Map<String, Value>), ProfileError> {
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if !(trimmed.is_empty() || trimmed.starts_with("//")) {
            break;
        }
        start += line.len();
    }
    let map: Map<String, Value> = serde_json::from_str(&text[start..])
        .map_err(|e| ProfileError::Unreadable(e.to_string()))?;
    let prefix = format!("{AUTOMATIC}/");
    if !map.keys().any(|k| k.starts_with(&prefix)) {
        return Err(ProfileError::NoAutomaticProfile);
    }
    Ok((start, map))
}

pub fn read(text: &str) -> Result<Snapshot, ProfileError> {
    let (_, map) = json_object(text)?;
    let get = |name: &str| map.get(&key(name));
    Ok(Snapshot {
        on: boolean(get(KEY_ON)),
        target: number(get(KEY_TARGET), Target::from_svp),
        exact: boolean(get(KEY_EXACT)),
        smoothness: number(get(KEY_SMOOTHNESS), Smoothness::from_svp),
        preset: number(get(KEY_PRESET), Preset::from_svp),
        masking: number(get(KEY_MASKING), Masking::from_svp),
        own_profiles: own_profiles(&map),
    })
}

fn boolean(value: Option<&Value>) -> Current<bool> {
    match value {
        None => Current::Missing,
        Some(Value::Bool(on)) => Current::Known(*on),
        Some(other) => Current::Custom(other.to_string()),
    }
}

fn number<T>(value: Option<&Value>, known: impl Fn(f64) -> Option<T>) -> Current<T> {
    match value {
        None => Current::Missing,
        Some(v) => v
            .as_f64()
            .and_then(known)
            .map_or_else(|| Current::Custom(v.to_string()), Current::Known),
    }
}

/// `P<id>/enable: true` profiles other than Automatic, by title.
fn own_profiles(map: &Map<String, Value>) -> Vec<String> {
    map.iter()
        .filter_map(|(k, v)| {
            let id = k.strip_suffix("/enable")?;
            (id != AUTOMATIC && id.starts_with('P') && !id.contains('/') && *v == Value::Bool(true))
                .then(|| {
                    map.get(&format!("{id}/title"))
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_string()
                })
        })
        .collect()
}

fn same(current: &Value, wanted: &Value) -> bool {
    match (current.as_f64(), wanted.as_f64()) {
        (Some(a), Some(b)) => (a - b).abs() < EPSILON,
        _ => current == wanted,
    }
}

/// `text` with the chosen values written into the Automatic profile, or
/// `None` when the override is off or they're already in place. Only the
/// value text of those keys' lines changes; every other byte stays.
pub fn patch(text: &str, overrides: &Overrides) -> Result<Option<String>, ProfileError> {
    if !overrides.enabled {
        return Ok(None);
    }
    let (start, map) = json_object(text)?;
    let wanted = overrides.wanted();
    let mut edits: Vec<(Range<usize>, &str)> = Vec::new();
    for (name, value) in &wanted {
        let key = key(name);
        let current = map.get(&key).ok_or(ProfileError::MissingKey(name))?;
        let new: Value = serde_json::from_str(value).map_err(|_| ProfileError::Check)?;
        if same(current, &new) {
            continue;
        }
        let range = value_range(text, start, &key)?;
        let on_line: Value = serde_json::from_str(&text[range.clone()])
            .map_err(|_| ProfileError::Layout(key.clone()))?;
        if on_line != *current {
            return Err(ProfileError::Layout(key));
        }
        edits.push((range, value));
    }
    if edits.is_empty() {
        return Ok(None);
    }
    // Back to front, so earlier ranges stay valid.
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut patched = text.to_string();
    for (range, value) in edits {
        patched.replace_range(range, value);
    }
    verify(&map, &patched, &wanted)?;
    Ok(Some(patched))
}

/// The byte range of `key`'s value on its `"key": value,` line.
fn value_range(text: &str, start: usize, key: &str) -> Result<Range<usize>, ProfileError> {
    let quoted = format!("\"{key}\"");
    let mut found = None;
    let mut offset = start;
    for line in text[start..].split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let body = content.trim_start();
        if let Some(rest) = body.strip_prefix(quoted.as_str()) {
            if found.is_some() {
                return Err(ProfileError::DuplicateKey(key.to_string()));
            }
            let value = rest
                .trim_start()
                .strip_prefix(':')
                .ok_or_else(|| ProfileError::Layout(key.to_string()))?
                .trim_start();
            let value_start = content.len() - value.len();
            let value = value.trim_end();
            let value = value.strip_suffix(',').unwrap_or(value).trim_end();
            if value.is_empty() {
                return Err(ProfileError::Layout(key.to_string()));
            }
            found = Some(offset + value_start..offset + value_start + value.len());
        }
        offset += line.len();
    }
    found.ok_or_else(|| ProfileError::Layout(key.to_string()))
}

/// The patched file must hold the same keys, the same values except the
/// wanted ones, and those as wanted.
fn verify(
    before: &Map<String, Value>,
    patched: &str,
    wanted: &[(&'static str, String)],
) -> Result<(), ProfileError> {
    let (_, after) = json_object(patched).map_err(|_| ProfileError::Check)?;
    if after.len() != before.len() {
        return Err(ProfileError::Check);
    }
    for (k, old) in before {
        let expected = match wanted.iter().find(|(name, _)| key(name) == *k) {
            Some((_, text)) => serde_json::from_str(text).map_err(|_| ProfileError::Check)?,
            None => old.clone(),
        };
        if !after.get(k).is_some_and(|new| same(new, &expected)) {
            return Err(ProfileError::Check);
        }
    }
    Ok(())
}

/// Where SVP may keep profiles.cfg, most likely first. Linux: the XDG data
/// folder. Windows (%APPDATA%, %LOCALAPPDATA%) and macOS (Application
/// Support, ~/.local/share) follow Qt's data folders.
pub fn candidates() -> Vec<PathBuf> {
    let Some(dirs) = directories::BaseDirs::new() else {
        return Vec::new();
    };
    let mut bases = vec![dirs.data_dir().to_path_buf()];
    if cfg!(windows) {
        bases.push(dirs.data_local_dir().to_path_buf());
    }
    if cfg!(target_os = "macos") {
        bases.push(dirs.home_dir().join(".local").join("share"));
    }
    bases
        .into_iter()
        .map(|base| base.join("SVP4").join("settings").join("profiles.cfg"))
        .collect()
}

pub fn profiles_path() -> Option<PathBuf> {
    candidates().into_iter().find(|path| path.is_file())
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

/// Writes the Preferences → SVP overrides into SVP's profiles.cfg. Called
/// just before this app starts SVP Manager (it reads the file only then).
pub fn apply_before_start() {
    let overrides = Settings::load().unwrap_or_default().frame_gen.svp_override;
    if !overrides.enabled || overrides.wanted().is_empty() {
        return;
    }
    let Some(path) = profiles_path() else {
        tracing::warn!(
            "SVP override not applied: profiles.cfg not found in {:?}",
            candidates()
        );
        return;
    };
    match apply_to(&path, &overrides) {
        Ok(true) => tracing::info!("SVP override written to {}", path.display()),
        Ok(false) => tracing::debug!("SVP override already in {}", path.display()),
        Err(e) => tracing::warn!("SVP override not applied to {}: {e:#}", path.display()),
    }
}

/// Patches `path` in place, keeping a one-time backup. `Ok(false)`: nothing
/// to change.
fn apply_to(path: &Path, overrides: &Overrides) -> anyhow::Result<bool> {
    use anyhow::Context;
    let text = std::fs::read_to_string(path).context("couldn't read it")?;
    let Some(patched) = patch(&text, overrides)? else {
        return Ok(false);
    };
    let backup = backup_path(path);
    if !backup.exists() {
        std::fs::copy(path, &backup)
            .with_context(|| format!("couldn't back it up to {}", backup.display()))?;
    }
    let permissions = std::fs::metadata(path).map(|m| m.permissions());
    crate::config::write_atomic(path, patched.as_bytes()).context("couldn't write it")?;
    if let Ok(permissions) = permissions {
        let _ = std::fs::set_permissions(path, permissions);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"// DO NOT MODIFY THIS FILE DIRECTLY.
// Use 'Application settings' -> 'Additional options' -> 'All settings' from SVP's main menu instead.

{
    "P10000001_1001_1001_1001_100000000001/fi_auto_adjust": 30,
    "P10000001_1001_1001_1001_100000000001/fi_masking": 100,
    "P10000001_1001_1001_1001_100000000001/fi_preset": 1,
    "P10000001_1001_1001_1001_100000000001/fi_target": 165,
    "P10000001_1001_1001_1001_100000000001/fi_target_exact": false,
    "P10000001_1001_1001_1001_100000000001/on": true,
    "P24e3c507_487a_483b_8af5_ee5f51c14924/conditions/00_frate_g": 60,
    "P24e3c507_487a_483b_8af5_ee5f51c14924/enable": true,
    "P24e3c507_487a_483b_8af5_ee5f51c14924/fi_masking": 100,
    "P24e3c507_487a_483b_8af5_ee5f51c14924/fi_target": 60,
    "P24e3c507_487a_483b_8af5_ee5f51c14924/on": false,
    "P24e3c507_487a_483b_8af5_ee5f51c14924/title": "Ignore HFR sources",
    "P74e92a6e_935e_46e2_985b_f5fcaddafdb9/enable": false,
    "P74e92a6e_935e_46e2_985b_f5fcaddafdb9/title": "RIFE AI engine",
    "predef/cast": false,
    "predef/ignore": true
}
"#;

    const AUTO: &str = "P10000001_1001_1001_1001_100000000001";

    fn chosen() -> Overrides {
        Overrides {
            enabled: true,
            ..Default::default()
        }
    }

    fn without_line(text: &str, needle: &str) -> String {
        text.lines()
            .filter(|l| !l.contains(needle))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[test]
    fn reads_the_automatic_profile() {
        let snapshot = read(SAMPLE).unwrap();
        assert_eq!(
            snapshot,
            Snapshot {
                on: Current::Known(true),
                target: Current::Known(Target::Fixed(165)),
                exact: Current::Known(false),
                smoothness: Current::Known(Smoothness(30)),
                preset: Current::Known(Preset::Animation),
                masking: Current::Known(Masking::Medium),
                own_profiles: vec!["Ignore HFR sources".to_string()],
            }
        );
    }

    #[test]
    fn unknown_values_read_as_custom() {
        let text = SAMPLE
            .replace(
                &format!("{AUTO}/fi_target\": 165"),
                &format!("{AUTO}/fi_target\": 2.75"),
            )
            .replace(
                &format!("{AUTO}/fi_masking\": 100"),
                &format!("{AUTO}/fi_masking\": 75"),
            )
            .replace(
                &format!("{AUTO}/fi_auto_adjust\": 30"),
                &format!("{AUTO}/fi_auto_adjust\": 10"),
            );
        let snapshot = read(&text).unwrap();
        assert_eq!(snapshot.target, Current::Custom("2.75".into()));
        assert_eq!(snapshot.masking, Current::Custom("75".into()));
        assert_eq!(snapshot.smoothness, Current::Custom("10".into()));

        let no_masking = without_line(SAMPLE, &format!("{AUTO}/fi_masking"));
        assert_eq!(read(&no_masking).unwrap().masking, Current::Missing);

        let text = SAMPLE.replace(
            &format!("{AUTO}/fi_target\": 165"),
            &format!("{AUTO}/fi_target\": 100"),
        );
        assert_eq!(read(&text).unwrap().target, Current::Custom("100".into()));
    }

    #[test]
    fn target_values_match_what_svp_writes() {
        let choices = Target::choices();
        let first: Vec<String> = choices[..9].iter().map(|t| t.svp_value()).collect();
        assert_eq!(
            first,
            [
                "1.0001", "2.0001", "2.5001", "3", "4", "5", "6", "0.5", "0.3333"
            ]
        );
        assert_eq!(choices.len(), 21);
        for t in &choices {
            assert_eq!(Target::from_svp(t.svp_value().parse().unwrap()), Some(*t));
        }
        assert_eq!(Target::from_svp(0.33333), Some(Target::ScreenThird));
        for v in [1.5, 7.0, 9.0, 100.0, 165.5, 500.0] {
            assert_eq!(Target::from_svp(v), None, "{v}");
        }
    }

    #[test]
    fn slider_preset_and_masking_round_trip() {
        for s in Smoothness::choices() {
            assert_eq!(Smoothness::from_svp(f64::from(s.svp_value())), Some(s));
        }
        for p in Preset::ALL {
            assert_eq!(Preset::from_svp(f64::from(p.svp_value())), Some(p));
        }
        for m in Masking::ALL {
            assert_eq!(Masking::from_svp(f64::from(m.svp_value())), Some(m));
        }
        let masking: Vec<u32> = Masking::ALL.iter().map(|m| m.svp_value()).collect();
        assert_eq!(masking, [0, 50, 100, 200]);
        assert_eq!(Smoothness::STOPS, [-50, -40, -30, -20, 0, 20, 30]);
        assert_eq!(Smoothness::from_svp(-10.0), None);
        assert_eq!(Masking::from_svp(75.0), None);
        assert_eq!(Preset::from_svp(2.0), None);
    }

    #[test]
    fn config_strings_round_trip() {
        for t in Target::choices() {
            assert_eq!(t.to_string().parse(), Ok(t));
        }
        for s in Smoothness::choices() {
            assert_eq!(s.to_string().parse(), Ok(s));
        }
        for p in Preset::ALL {
            assert_eq!(p.to_string().parse(), Ok(p));
        }
        for m in Masking::ALL {
            assert_eq!(m.to_string().parse(), Ok(m));
        }
        assert!("100".parse::<Target>().is_err());
        assert!("10".parse::<Smoothness>().is_err());

        let overrides = Overrides {
            enabled: true,
            target: Some(Target::Fixed(60)),
            exact: Some(true),
            smoothness: Some(Smoothness(-20)),
            masking: Some(Masking::High),
            ..Default::default()
        };
        let text = toml::to_string(&overrides).unwrap();
        assert_eq!(toml::from_str::<Overrides>(&text).unwrap(), overrides);

        let lenient: Overrides = toml::from_str(
            "enabled = true\ntarget = \"x9\"\nmasking = \"huge\"\nsmoothness = \"7\"\n",
        )
        .unwrap();
        assert!(lenient.enabled);
        assert_eq!(lenient.target, None);
        assert_eq!(lenient.masking, None);
        assert_eq!(lenient.smoothness, None);
        assert!(Overrides::default().is_unset());
    }

    #[test]
    fn patch_changes_only_the_values() {
        let overrides = Overrides {
            target: Some(Target::Movie2),
            exact: Some(true),
            smoothness: Some(Smoothness(-50)),
            masking: Some(Masking::High),
            ..chosen()
        };
        let text = patch(SAMPLE, &overrides).unwrap().unwrap();
        let changed: Vec<&str> = text
            .lines()
            .zip(SAMPLE.lines())
            .filter(|(a, b)| a != b)
            .map(|(a, _)| a)
            .collect();
        assert_eq!(text.lines().count(), SAMPLE.lines().count());
        assert_eq!(
            changed,
            [
                format!("    \"{AUTO}/fi_auto_adjust\": -50,"),
                format!("    \"{AUTO}/fi_masking\": 200,"),
                format!("    \"{AUTO}/fi_target\": 2.0001,"),
                format!("    \"{AUTO}/fi_target_exact\": true,"),
            ]
        );
        assert!(text.contains("\"P24e3c507_487a_483b_8af5_ee5f51c14924/fi_target\": 60,"));
    }

    #[test]
    fn nothing_to_write() {
        let values = Overrides {
            target: Some(Target::Movie2),
            ..Default::default()
        };
        assert_eq!(patch(SAMPLE, &values), Ok(None));
        assert_eq!(patch(SAMPLE, &chosen()), Ok(None));
        let same = Overrides {
            on: Some(true),
            target: Some(Target::Fixed(165)),
            exact: Some(false),
            smoothness: Some(Smoothness(30)),
            preset: Some(Preset::Animation),
            masking: Some(Masking::Medium),
            ..chosen()
        };
        assert_eq!(patch(SAMPLE, &same), Ok(None));
    }

    #[test]
    fn keeps_crlf_line_endings() {
        let crlf = SAMPLE.replace('\n', "\r\n");
        let overrides = Overrides {
            target: Some(Target::Movie3),
            ..chosen()
        };
        let result = patch(&crlf, &overrides).unwrap().unwrap();
        assert!(result.contains("fi_target\": 3,\r\n"));
        assert_eq!(result.matches('\n').count(), result.matches("\r\n").count());
    }

    #[test]
    fn last_entry_without_comma() {
        let text = format!("{{\n    \"{AUTO}/fi_target\": 165\n}}\n");
        let overrides = Overrides {
            target: Some(Target::Movie4),
            ..chosen()
        };
        assert_eq!(
            patch(&text, &overrides).unwrap().unwrap(),
            format!("{{\n    \"{AUTO}/fi_target\": 4\n}}\n")
        );
    }

    #[test]
    fn refuses_what_it_cannot_patch_safely() {
        let movie2 = Overrides {
            target: Some(Target::Movie2),
            ..chosen()
        };
        assert!(matches!(
            patch("", &movie2),
            Err(ProfileError::Unreadable(_))
        ));
        assert!(matches!(
            patch("garbage", &movie2),
            Err(ProfileError::Unreadable(_))
        ));

        let other_id = SAMPLE.replace(AUTO, "P20000001_1001_1001_1001_100000000001");
        assert_eq!(
            patch(&other_id, &movie2),
            Err(ProfileError::NoAutomaticProfile)
        );

        let no_masking = without_line(SAMPLE, &format!("{AUTO}/fi_masking"));
        let high = Overrides {
            masking: Some(Masking::High),
            ..chosen()
        };
        assert_eq!(
            patch(&no_masking, &high),
            Err(ProfileError::MissingKey("fi_masking"))
        );
        assert!(matches!(patch(&no_masking, &movie2), Ok(Some(_))));

        let line = format!("    \"{AUTO}/fi_target\": 165,");
        let duplicated = SAMPLE.replace(&line, &format!("{line}\n{line}"));
        assert!(patch(&duplicated, &movie2).is_err());

        let split = SAMPLE.replace("fi_target\": 165,", "fi_target\":\n        165,");
        assert!(patch(&split, &movie2).is_err());
    }

    #[test]
    fn backup_sits_beside_the_file() {
        assert_eq!(
            backup_path(&Path::new("/x").join("profiles.cfg")),
            Path::new("/x").join("profiles.cfg.embyclientplus-bak")
        );
    }

    #[test]
    fn apply_writes_once_backed_up() {
        let dir = std::env::temp_dir().join(format!("ecp-svp-profile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profiles.cfg");
        std::fs::write(&path, SAMPLE).unwrap();
        let backup = backup_path(&path);

        let movie2 = Overrides {
            target: Some(Target::Movie2),
            ..chosen()
        };
        assert!(apply_to(&path, &movie2).unwrap());
        assert!(std::fs::read_to_string(&path).unwrap().contains("2.0001"));
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), SAMPLE);

        let movie3 = Overrides {
            target: Some(Target::Movie3),
            ..chosen()
        };
        assert!(apply_to(&path, &movie3).unwrap());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), SAMPLE);
        assert!(!apply_to(&path, &movie3).unwrap());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
