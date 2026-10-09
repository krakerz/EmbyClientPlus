//! Local files and links for the standalone player (`--player`): what to
//! call them, and the videos before and after one in its folder.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::emby::models::BaseItem;
use crate::playback::LocalMedia;

/// Files the folder walk treats as videos.
const VIDEO_EXTENSIONS: [&str; 14] = [
    "mkv", "mp4", "m4v", "avi", "mov", "webm", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg", "ogv",
    "3gp",
];

/// `target` (a path or URL) ready to play, with its folder neighbours.
pub fn media(target: &str) -> LocalMedia {
    let path = Path::new(target);
    if !is_url(target) && path.is_file() {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let siblings = videos_beside(&path);
        let index = siblings.iter().position(|p| *p == path);
        let neighbour = |offset: isize| {
            let i = index? as isize + offset;
            siblings.get(usize::try_from(i).ok()?).map(|p| item_for(p))
        };
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        return LocalMedia {
            target: path.to_string_lossy().into_owned(),
            item: item_for(&path),
            previous: neighbour(-1),
            next: neighbour(1),
            override_key: format!("local:{}", folder.to_string_lossy()),
            client: None,
            // `Movie.eng.srt` and the like beside the file.
            subtitle_files: crate::downloads::subtitles_beside(&path)
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        };
    }
    LocalMedia {
        target: target.to_string(),
        item: BaseItem {
            id: format!("url:{target}"),
            name: link_name(target),
            item_type: "Video".into(),
            ..Default::default()
        },
        previous: None,
        next: None,
        override_key: format!("url:{target}"),
        client: None,
        subtitle_files: Vec::new(),
    }
}

/// The file an item from [`media`] stands for.
pub fn path_of(item: &BaseItem) -> Option<&str> {
    item.id.strip_prefix("local:")
}

fn item_for(path: &Path) -> BaseItem {
    BaseItem {
        id: format!("local:{}", path.to_string_lossy()),
        name: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        item_type: "Video".into(),
        ..Default::default()
    }
}

fn is_url(target: &str) -> bool {
    target.contains("://")
}

/// The last part of a link's path, else the whole link.
fn link_name(url: &str) -> String {
    url.split(['?', '#'])
        .next()
        .and_then(|u| u.trim_end_matches('/').rsplit('/').next())
        .filter(|s| !s.is_empty() && !s.contains(':'))
        .map(str::to_string)
        .unwrap_or_else(|| url.to_string())
}

/// The videos in `path`'s folder, in natural order (2 before 10).
fn videos_beside(path: &Path) -> Vec<PathBuf> {
    let Some(folder) = path.parent() else {
        return Vec::new();
    };
    let mut videos: Vec<PathBuf> = std::fs::read_dir(folder)
        .map(|entries| entries.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    videos.retain(|p| p.is_file() && is_video(p));
    videos.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    videos
}

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| VIDEO_EXTENSIONS.contains(&e.as_str()))
}

/// Compares names with runs of digits as numbers, ignoring case.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (da, db) = (take(&mut a), take(&mut b));
                let (ta, tb) = (da.trim_start_matches('0'), db.trim_start_matches('0'));
                let order = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut names = vec!["Ep 10.mkv", "ep 2.mkv", "Ep 1.mkv", "Ep 02b.mkv"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["Ep 1.mkv", "ep 2.mkv", "Ep 02b.mkv", "Ep 10.mkv"]);
    }

    #[test]
    fn folder_neighbours() {
        let dir = std::env::temp_dir().join(format!("ecp-local-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["Show 01.mkv", "Show 02.mkv", "Show 10.mkv", "notes.txt"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let media = media(&dir.join("Show 02.mkv").to_string_lossy());
        assert_eq!(media.item.name, "Show 02");
        assert_eq!(media.previous.unwrap().name, "Show 01");
        let next = media.next.unwrap();
        assert_eq!(next.name, "Show 10");
        assert!(path_of(&next).unwrap().ends_with("Show 10.mkv"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn links_are_named_by_their_last_part() {
        assert_eq!(link_name("https://x.org/videos/clip.mp4?t=3"), "clip.mp4");
        assert_eq!(link_name("https://www.youtube.com/watch?v=abc"), "watch");
    }
}
