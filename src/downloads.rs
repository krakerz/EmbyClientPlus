//! Titles downloaded for offline playback: the original file, Emby's
//! metadata beside it (`<file>.emby.json`), its artwork (`<file>.jpg`) and
//! its external subtitles (`<name>.<language>[.forced].<format>`, the
//! naming mpv reads language and forced from).
//! The folder is the index: whatever has a `.emby.json` is a download.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::emby::EmbyClient;
use crate::emby::models::BaseItem;
use crate::update::Progress;

const META_SUFFIX: &str = ".emby.json";

/// A downloaded title.
#[derive(Debug, Clone)]
pub struct Download {
    pub item: BaseItem,
    pub file: PathBuf,
    pub image: Option<PathBuf>,
}

impl Download {
    pub fn size(&self) -> u64 {
        std::fs::metadata(&self.file).map_or(0, |m| m.len())
    }
}

/// Where downloads go: Preferences, else `~/Videos/Emby Client+`.
pub fn folder() -> PathBuf {
    let chosen = crate::config::Settings::load()
        .unwrap_or_default()
        .downloads
        .folder;
    if let Some(folder) = chosen.filter(|f| !f.is_empty()) {
        return PathBuf::from(folder);
    }
    let videos = directories::UserDirs::new()
        .and_then(|dirs| dirs.video_dir().map(Path::to_path_buf))
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().join("Videos")))
        .unwrap_or_else(|| PathBuf::from("."));
    videos.join(crate::APP_NAME)
}

/// Everything downloaded, series by series in episode order, then movies.
pub fn list() -> Vec<Download> {
    let mut found = Vec::new();
    collect(&folder(), &mut found, 0);
    found.sort_by_key(|d| sort_key(&d.item));
    found
}

fn sort_key(item: &BaseItem) -> (String, i32, i32, String) {
    (
        item.series_name
            .clone()
            .unwrap_or_else(|| item.name.clone())
            .to_lowercase(),
        item.parent_index_number.unwrap_or(0),
        item.index_number.unwrap_or(0),
        item.name.to_lowercase(),
    )
}

fn collect(dir: &Path, found: &mut Vec<Download>, depth: usize) {
    // Series/Season/files: a few levels is plenty.
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
        if path.is_dir() {
            collect(&path, found, depth + 1);
        } else if let Some(download) = read(&path) {
            found.push(download);
        }
    }
}

/// The download whose metadata is `meta`, if its file is there too.
fn read(meta: &Path) -> Option<Download> {
    let name = meta.file_name()?.to_str()?;
    let file = meta.with_file_name(name.strip_suffix(META_SUFFIX)?);
    if !file.is_file() {
        return None;
    }
    let item: BaseItem = serde_json::from_str(&std::fs::read_to_string(meta).ok()?).ok()?;
    let image = Some(image_path(&file)).filter(|p| p.is_file());
    Some(Download { item, file, image })
}

fn meta_path(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(META_SUFFIX);
    file.with_file_name(name)
}

fn image_path(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(".jpg");
    file.with_file_name(name)
}

/// A download still to do, saved so it survives the app closing (even
/// abruptly): queued ones carry on at the next start, paused ones wait.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Pending {
    pub user_id: String,
    pub id: String,
    pub name: String,
    pub item_type: String,
    pub series_id: Option<String>,
    pub series_name: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub paused: bool,
}

impl Pending {
    pub fn new(user_id: &str, item: &BaseItem, paused: bool) -> Self {
        Pending {
            user_id: user_id.to_string(),
            id: item.id.clone(),
            name: item.name.clone(),
            item_type: item.item_type.clone(),
            series_id: item.series_id.clone(),
            series_name: item.series_name.clone(),
            season: item.parent_index_number,
            episode: item.index_number,
            paused,
        }
    }

    /// Enough of the item to queue and label it.
    pub fn item(&self) -> BaseItem {
        BaseItem {
            id: self.id.clone(),
            name: self.name.clone(),
            item_type: self.item_type.clone(),
            series_id: self.series_id.clone(),
            series_name: self.series_name.clone(),
            parent_index_number: self.season,
            index_number: self.episode,
            ..Default::default()
        }
    }
}

fn pending_path() -> Result<PathBuf> {
    Ok(crate::config::config_dir()?.join("download-queue.json"))
}

fn load_all_pending() -> Vec<Pending> {
    pending_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// `user_id`'s saved downloads, in order.
pub fn load_pending(user_id: &str) -> Vec<Pending> {
    load_all_pending()
        .into_iter()
        .filter(|p| p.user_id == user_id)
        .collect()
}

/// Replaces `user_id`'s saved downloads with `pending`.
pub fn save_pending(user_id: &str, pending: &[Pending]) {
    let mut all = load_all_pending();
    all.retain(|p| p.user_id != user_id);
    all.extend(pending.iter().cloned());
    let result = (|| -> Result<()> {
        let path = pending_path()?;
        std::fs::write(&path, serde_json::to_string_pretty(&all)?)
            .with_context(|| format!("writing {}", path.display()))
    })();
    if let Err(e) = result {
        tracing::warn!("couldn't save the download queue: {e:#}");
    }
}

/// The downloaded items' ids, rescanned at most every few seconds (and
/// after each download or delete), so lists can badge every card cheaply.
static IDS: Mutex<Option<(Instant, Arc<HashSet<String>>)>> = Mutex::new(None);
const IDS_FRESH_FOR: Duration = Duration::from_secs(5);

/// Whether the Emby item `item_id` is downloaded.
pub fn is_downloaded(item_id: &str) -> bool {
    let ids = {
        let mut cached = IDS.lock().unwrap_or_else(|e| e.into_inner());
        match cached.as_ref() {
            Some((at, ids)) if at.elapsed() < IDS_FRESH_FOR => ids.clone(),
            _ => {
                let ids: Arc<HashSet<String>> =
                    Arc::new(list().into_iter().map(|d| d.item.id).collect());
                *cached = Some((Instant::now(), ids.clone()));
                ids
            }
        }
    };
    ids.contains(item_id)
}

fn forget_ids() {
    IDS.lock().unwrap_or_else(|e| e.into_inner()).take();
}

/// The download of the Emby item `item_id`, if there is one.
pub fn find(item_id: &str) -> Option<Download> {
    list().into_iter().find(|d| d.item.id == item_id)
}

/// The downloaded episodes before and after `download` in its series.
pub fn neighbours(download: &Download) -> (Option<Download>, Option<Download>) {
    let Some(series) = &download.item.series_id else {
        return (None, None);
    };
    let episodes: Vec<Download> = list()
        .into_iter()
        .filter(|d| d.item.series_id.as_ref() == Some(series))
        .collect();
    let Some(index) = episodes.iter().position(|d| d.item.id == download.item.id) else {
        return (None, None);
    };
    let previous = index.checked_sub(1).and_then(|i| episodes.get(i).cloned());
    (previous, episodes.get(index + 1).cloned())
}

/// Where `item` goes under the downloads folder, without its extension:
/// `Series/Season 1/S01E04 - Name`, or `Name (Year)` for anything else.
fn relative_path(item: &BaseItem) -> PathBuf {
    let clean = |text: &str| -> String {
        text.chars()
            .map(|c| {
                if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                    '_'
                } else {
                    c
                }
            })
            .collect::<String>()
            .trim()
            .trim_end_matches('.')
            .to_string()
    };
    match (
        &item.series_name,
        item.parent_index_number,
        item.index_number,
    ) {
        (Some(series), season, Some(episode)) if item.item_type == "Episode" => {
            let season = season.unwrap_or(0);
            PathBuf::from(clean(series))
                .join(format!("Season {season}"))
                .join(clean(&format!("S{season:02}E{episode:02} - {}", item.name)))
        }
        _ => PathBuf::from(clean(&match item.production_year {
            Some(year) => format!("{} ({year})", item.name),
            None => item.name.clone(),
        })),
    }
}

/// Not enough room on the disk for a download (it never started).
#[derive(Debug)]
pub struct NotEnoughSpace {
    pub needed: u64,
    pub free: u64,
}

impl std::fmt::Display for NotEnoughSpace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "not enough disk space: needs {:.1} GB, {:.1} GB free",
            self.needed as f64 / 1e9,
            self.free as f64 / 1e9
        )
    }
}

impl std::error::Error for NotEnoughSpace {}

/// Kept free beside downloads, so the disk never fills up completely.
const SPACE_RESERVE: u64 = 1_000_000_000;

/// Free bytes on the disk holding `path` (or its nearest existing parent).
pub fn free_space(path: &Path) -> Option<u64> {
    let existing = path.ancestors().find(|p| p.exists())?;
    free_space_at(existing)
}

#[cfg(unix)]
fn free_space_at(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string and a zeroed struct for the call to fill.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(stats.f_bavail as u64 * stats.f_frsize as u64)
}

#[cfg(windows)]
fn free_space_at(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free: u64 = 0;
    // SAFETY: a NUL-terminated path; the other outputs may be null.
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

/// A half-finished download's file: `<file>.<item id>.part`, so it can be
/// found again (to resume or discard) from the item alone.
fn partial_path(file: &Path, item_id: &str) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{item_id}.part"));
    file.with_file_name(name)
}

/// Deletes `item_id`'s half-finished download, if any (a cancelled one).
pub fn discard_partial(item_id: &str) {
    let suffix = format!(".{item_id}.part");
    fn walk(dir: &Path, suffix: &str, depth: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
            if path.is_dir() && depth < 4 {
                walk(&path, suffix, depth + 1);
            } else if path.to_string_lossy().ends_with(suffix) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    walk(&folder(), &suffix, 0);
}

/// Downloads `item_id`'s original file with its metadata and artwork;
/// returns where it went. `progress` and `cancelled` are shared with the
/// UI. A cancelled or interrupted download keeps its partial file, which
/// the next attempt continues; `discard` (a cancel, not a pause) deletes it.
pub async fn fetch(
    client: Arc<EmbyClient>,
    user_id: String,
    item_id: String,
    progress: Arc<Progress>,
    cancelled: Arc<AtomicBool>,
    discard: Arc<AtomicBool>,
) -> Result<PathBuf> {
    let raw = client.item_json(&user_id, &item_id).await?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).context("Emby sent unreadable metadata")?;
    let item: BaseItem =
        serde_json::from_value(value.clone()).context("Emby sent unreadable metadata")?;
    let source = &value["MediaSources"][0];
    let extension = source["Container"]
        .as_str()
        .and_then(|c| c.split(',').next())
        .or_else(|| {
            source["Path"]
                .as_str()
                .and_then(|p| Path::new(p).extension()?.to_str())
        })
        .unwrap_or("mkv")
        .to_string();
    let source_id = source["Id"].as_str().unwrap_or(&item.id).to_string();

    let base = folder().join(relative_path(&item));
    let file = base.with_file_name(format!(
        "{}.{extension}",
        base.file_name().unwrap_or_default().to_string_lossy()
    ));
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("couldn't create {}", parent.display()))?;
    }
    // Written as .part and renamed at the end, so a half download is never
    // mistaken for a finished one.
    let partial = partial_path(&file, &item.id);
    let have = std::fs::metadata(&partial).map_or(0, |m| m.len());
    if let (Some(size), Some(free)) = (source["Size"].as_u64(), free_space(&file)) {
        let needed = size.saturating_sub(have);
        if needed + SPACE_RESERVE > free {
            return Err(NotEnoughSpace { needed, free }.into());
        }
    }
    let url = client.direct_stream_url(&item.id, &source_id)?;
    if let Err(e) = client
        .download_to(&url, &partial, &progress, &cancelled, true)
        .await
    {
        if discard.load(std::sync::atomic::Ordering::Relaxed) {
            let _ = std::fs::remove_file(&partial);
        }
        return Err(e);
    }
    std::fs::rename(&partial, &file)
        .with_context(|| format!("couldn't finish {}", file.display()))?;
    std::fs::write(meta_path(&file), &raw)
        .with_context(|| format!("couldn't write the metadata for {}", file.display()))?;
    forget_ids();
    save_subtitles(&client, &item.id, &source_id, source, &file).await;
    if let Some(image) = item.landscape().or_else(|| item.poster()) {
        let url = client.url(&EmbyClient::image_path(&image, 640));
        let art = Progress::default();
        // Artwork is a nicety: a download without it still plays.
        if let Err(e) = client
            .download_to(
                &url,
                &image_path(&file),
                &art,
                &AtomicBool::new(false),
                false,
            )
            .await
        {
            tracing::warn!("couldn't save the artwork for {}: {e:#}", file.display());
        }
    }
    Ok(file)
}

/// Subtitle formats saved beside downloads (and picked up for playback).
const SUBTITLE_EXTENSIONS: [&str; 5] = ["srt", "ass", "ssa", "vtt", "sub"];

/// Saves `source`'s external subtitles beside `file`. Embedded ones are
/// already inside the file. A subtitle that fails is skipped: the video
/// still plays.
async fn save_subtitles(
    client: &EmbyClient,
    item_id: &str,
    source_id: &str,
    source: &serde_json::Value,
    file: &Path,
) {
    let streams = source["MediaStreams"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut used: Vec<PathBuf> = Vec::new();
    for stream in streams.iter().filter(|s| {
        s["Type"].as_str() == Some("Subtitle") && s["IsExternal"].as_bool() == Some(true)
    }) {
        let Some(index) = stream["Index"].as_i64().and_then(|i| i32::try_from(i).ok()) else {
            continue;
        };
        let format = match stream["Codec"]
            .as_str()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("ass") => "ass",
            Some("ssa") => "ssa",
            Some("webvtt" | "vtt") => "vtt",
            _ => "srt",
        };
        let language = stream["Language"].as_str().unwrap_or("und");
        let forced = stream["IsForced"].as_bool() == Some(true);
        let path = subtitle_path(file, language, forced, format, &used);
        let url = match client.subtitle_url(item_id, source_id, index, format) {
            Ok(url) => url,
            Err(e) => {
                tracing::warn!("no subtitle url: {e:#}");
                continue;
            }
        };
        let done = Progress::default();
        match client
            .download_to(&url, &path, &done, &AtomicBool::new(false), false)
            .await
        {
            Ok(()) => used.push(path),
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                tracing::warn!(
                    "couldn't save subtitle {index} of {}: {e:#}",
                    file.display()
                );
            }
        }
    }
}

/// `<name>.<language>[.forced][.2].<format>` beside `file`, not one of `used`.
fn subtitle_path(
    file: &Path,
    language: &str,
    forced: bool,
    format: &str,
    used: &[PathBuf],
) -> PathBuf {
    let stem = file.file_stem().unwrap_or_default().to_string_lossy();
    let language: String = language
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_lowercase();
    let mut base = format!(
        "{stem}.{}",
        if language.is_empty() {
            "und"
        } else {
            &language
        }
    );
    if forced {
        base.push_str(".forced");
    }
    (1..)
        .map(|n| {
            let name = if n == 1 {
                format!("{base}.{format}")
            } else {
                format!("{base}.{n}.{format}")
            };
            file.with_file_name(name)
        })
        .find(|path| !used.contains(path))
        .expect("an unused name")
}

/// The subtitle files saved beside `file` (see [`save_subtitles`]).
pub fn subtitles_beside(file: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (file.parent(), file.file_stem()) else {
        return Vec::new();
    };
    let prefix = format!("{}.", stem.to_string_lossy());
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    found.retain(|path| {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        name.starts_with(&prefix)
            && path.extension().is_some_and(|ext| {
                SUBTITLE_EXTENSIONS.contains(&ext.to_string_lossy().to_lowercase().as_str())
            })
    });
    found.sort();
    found
}

/// Removes a download: its file, metadata, artwork and subtitles, and
/// folders left empty.
pub fn delete(download: &Download) -> Result<()> {
    let subtitles = subtitles_beside(&download.file);
    std::fs::remove_file(&download.file)
        .with_context(|| format!("couldn't delete {}", download.file.display()))?;
    let _ = std::fs::remove_file(meta_path(&download.file));
    forget_ids();
    let _ = std::fs::remove_file(image_path(&download.file));
    for subtitle in subtitles {
        let _ = std::fs::remove_file(subtitle);
    }
    // Season, then series folder, when nothing else is in them.
    let root = folder();
    let mut dir = download.file.parent().map(Path::to_path_buf);
    while let Some(current) = dir.filter(|d| *d != root && d.starts_with(&root)) {
        if std::fs::remove_dir(&current).is_err() {
            break;
        }
        dir = current.parent().map(Path::to_path_buf);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episodes_and_movies_get_tidy_paths() {
        let episode = BaseItem {
            name: "Who: Me?".into(),
            item_type: "Episode".into(),
            series_name: Some("Show".into()),
            parent_index_number: Some(1),
            index_number: Some(4),
            ..Default::default()
        };
        assert_eq!(
            relative_path(&episode),
            PathBuf::from("Show/Season 1/S01E04 - Who_ Me_")
        );
        let movie = BaseItem {
            name: "Film".into(),
            item_type: "Movie".into(),
            production_year: Some(2020),
            ..Default::default()
        };
        assert_eq!(relative_path(&movie), PathBuf::from("Film (2020)"));
    }

    #[test]
    fn subtitles_are_named_for_mpv_and_found_again() {
        let dir = std::env::temp_dir().join(format!("ecp-subs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("S01E01 - Pilot.mkv");
        let first = subtitle_path(&file, "eng", false, "srt", &[]);
        assert_eq!(first, dir.join("S01E01 - Pilot.eng.srt"));
        assert_eq!(
            subtitle_path(&file, "eng", false, "srt", std::slice::from_ref(&first)),
            dir.join("S01E01 - Pilot.eng.2.srt")
        );
        assert_eq!(
            subtitle_path(&file, "en-US", true, "ass", &[]),
            dir.join("S01E01 - Pilot.enus.forced.ass")
        );
        for name in [
            "S01E01 - Pilot.mkv",
            "S01E01 - Pilot.eng.srt",
            "S01E01 - Pilot.jpn.forced.ass",
            "S01E01 - Pilot.mkv.jpg",
            "S01E01 - Pilot 2.eng.srt",
        ] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        assert_eq!(
            subtitles_beside(&file),
            [
                dir.join("S01E01 - Pilot.eng.srt"),
                dir.join("S01E01 - Pilot.jpn.forced.ass")
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn partial_files_carry_the_item_id() {
        let file = PathBuf::from("/x/Show/Season 1/S01E01 - Pilot.mkv");
        assert_eq!(
            partial_path(&file, "42"),
            PathBuf::from("/x/Show/Season 1/S01E01 - Pilot.mkv.42.part")
        );
        assert!(free_space(Path::new("/definitely/not/here")).is_some_and(|f| f > 0));
    }

    #[test]
    fn pending_downloads_keep_enough_to_queue_them() {
        let item = BaseItem {
            id: "7".into(),
            name: "Pilot".into(),
            item_type: "Episode".into(),
            series_name: Some("Show".into()),
            parent_index_number: Some(1),
            index_number: Some(2),
            ..Default::default()
        };
        let pending = Pending::new("u", &item, true);
        let back: Pending =
            serde_json::from_str(&serde_json::to_string(&pending).unwrap()).unwrap();
        let again = back.item();
        assert!(back.paused);
        assert_eq!((again.id.as_str(), again.index_number), ("7", Some(2)));
        assert_eq!(again.series_name.as_deref(), Some("Show"));
    }

    #[test]
    fn metadata_beside_a_file_makes_a_download() {
        let dir = std::env::temp_dir().join(format!("ecp-downloads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("S01E01 - Pilot.mkv");
        std::fs::write(&file, "video").unwrap();
        std::fs::write(
            meta_path(&file),
            r#"{"Id":"42","Name":"Pilot","Type":"Episode","SeriesId":"7"}"#,
        )
        .unwrap();
        // Metadata without its file is ignored.
        std::fs::write(dir.join("gone.mkv.emby.json"), r#"{"Id":"1","Name":"x"}"#).unwrap();
        let mut found = Vec::new();
        collect(&dir, &mut found, 0);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].item.id, "42");
        assert_eq!(found[0].size(), 5);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
