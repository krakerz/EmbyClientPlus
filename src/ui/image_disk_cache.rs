//! On-disk copies of server images, so posters show instantly across
//! launches. Image URLs carry the image's tag, so a changed image gets a
//! new key and stale files simply age out.

use std::collections::hash_map::DefaultHasher;
use std::fs::FileTimes;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Oldest-used files are deleted past this total size.
const MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Tags already catch changed images; this just re-fetches anything older
/// so a bad or odd copy can't live forever.
const REFRESH_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

fn dir() -> Option<PathBuf> {
    crate::config::cache_dir().map(|dir| dir.join("images"))
}

/// Cache file for an image URL (full URL, so different servers never mix).
pub fn file_for(url: &str) -> Option<PathBuf> {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    Some(dir()?.join(format!("{:016x}", hasher.finish())))
}

/// A cached image; `fresh` is false once it is old enough to re-fetch.
pub struct Cached {
    pub bytes: Vec<u8>,
    pub fresh: bool,
}

/// Modified time is when the file was fetched, accessed time its last use.
fn is_fresh(modified: SystemTime, now: SystemTime) -> bool {
    // A modified time in the future (clock change) counts as fresh.
    now.duration_since(modified)
        .is_ok_and(|age| age < REFRESH_AFTER)
        || modified > now
}

pub fn read(url: &str) -> Option<Cached> {
    let path = file_for(url)?;
    let bytes = std::fs::read(&path).ok()?;
    let fresh = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .is_ok_and(|modified| is_fresh(modified, SystemTime::now()));
    // Pruning goes by accessed time, set explicitly because many mounts
    // don't update it; without it the most-viewed images would go first.
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(&path) {
        let _ = file.set_times(FileTimes::new().set_accessed(SystemTime::now()));
    }
    Some(Cached { bytes, fresh })
}

pub fn write(url: &str, bytes: &[u8]) {
    let Some(path) = file_for(url) else { return };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    // Write-then-rename so a crash never leaves a truncated image behind.
    let partial = path.with_extension("partial");
    if std::fs::write(&partial, bytes).is_ok() {
        let _ = std::fs::rename(&partial, &path);
    }
}

/// Trims the cache to [`MAX_BYTES`], least recently used first. Run once at startup.
pub fn prune() {
    if let Some(dir) = dir() {
        prune_in(&dir, MAX_BYTES);
    }
}

fn prune_in(dir: &Path, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(|meta| meta.is_file())?;
            let used = meta.accessed().or_else(|_| meta.modified()).ok()?;
            Some((used, meta.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= max_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

/// Bytes the cache takes on disk.
pub fn size() -> u64 {
    dir().map_or(0, |dir| size_in(&dir))
}

fn size_in(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok().filter(|meta| meta.is_file()))
        .map(|meta| meta.len())
        .sum()
}

/// Deletes every cached image (and leftover partial downloads).
pub fn clear() {
    if let Some(dir) = dir() {
        clear_in(&dir);
    }
}

fn clear_in(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        if entry.metadata().is_ok_and(|meta| meta.is_file()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "embyclientplus-image-cache-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn keys_differ_per_url() {
        let a = file_for("http://a/emby/Items/1/Images/Primary?tag=x");
        let b = file_for("http://b/emby/Items/1/Images/Primary?tag=x");
        assert_ne!(a, b);
        assert_eq!(a, file_for("http://a/emby/Items/1/Images/Primary?tag=x"));
    }

    #[test]
    fn fresh_until_the_refresh_age() {
        let now = SystemTime::now();
        assert!(is_fresh(now, now));
        assert!(is_fresh(now - REFRESH_AFTER + Duration::from_secs(60), now));
        assert!(!is_fresh(now - REFRESH_AFTER, now));
        assert!(!is_fresh(now - REFRESH_AFTER * 2, now));
        assert!(is_fresh(now + Duration::from_secs(3600), now));
    }

    #[test]
    fn size_and_clear_cover_every_file_and_keep_the_dir() {
        let dir = temp_dir("clear");
        std::fs::write(dir.join("a"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("b.partial"), [0u8; 5]).unwrap();
        assert_eq!(size_in(&dir), 15);
        clear_in(&dir);
        assert_eq!(size_in(&dir), 0);
        assert!(dir.is_dir());
        assert_eq!(size_in(&dir.join("missing")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_drops_the_least_recently_used_first() {
        let dir = temp_dir("prune");
        let now = SystemTime::now();
        // "old" was fetched most recently but used longest ago.
        for (name, used_ago) in [("old", 300), ("mid", 200), ("new", 100)] {
            let path = dir.join(name);
            std::fs::write(&path, [0u8; 10]).unwrap();
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            let used = now - Duration::from_secs(used_ago);
            file.set_times(FileTimes::new().set_accessed(used).set_modified(now))
                .unwrap();
        }
        prune_in(&dir, 20);
        assert!(!dir.join("old").exists());
        assert!(dir.join("mid").exists() && dir.join("new").exists());
        prune_in(&dir, 0);
        assert_eq!(size_in(&dir), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
