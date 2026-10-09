//! Titles downloaded for offline playback: the original file, Emby's
//! metadata beside it (`<file>.emby.json`), its artwork (`<file>.jpg`) and
//! its external subtitles (`<name>.<language>[.forced].<format>`, the
//! naming mpv reads language and forced from).
//! The folder is the index: whatever has a `.emby.json` is a download.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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

/// Downloads `item_id`'s original file with its metadata and artwork;
/// returns where it went. `progress` and `cancelled` are shared with the UI.
pub async fn fetch(
    client: Arc<EmbyClient>,
    user_id: String,
    item_id: String,
    progress: Arc<Progress>,
    cancelled: Arc<AtomicBool>,
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
    let partial = file.with_extension(format!("{extension}.part"));
    let url = client.direct_stream_url(&item.id, &source_id)?;
    if let Err(e) = client
        .download_to(&url, &partial, &progress, &cancelled)
        .await
    {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    std::fs::rename(&partial, &file)
        .with_context(|| format!("couldn't finish {}", file.display()))?;
    std::fs::write(meta_path(&file), &raw)
        .with_context(|| format!("couldn't write the metadata for {}", file.display()))?;
    save_subtitles(&client, &item.id, &source_id, source, &file).await;
    if let Some(image) = item.landscape().or_else(|| item.poster()) {
        let url = client.url(&EmbyClient::image_path(&image, 640));
        let art = Progress::default();
        // Artwork is a nicety: a download without it still plays.
        if let Err(e) = client
            .download_to(&url, &image_path(&file), &art, &AtomicBool::new(false))
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
            .download_to(&url, &path, &done, &AtomicBool::new(false))
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
