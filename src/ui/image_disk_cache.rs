//! On-disk copies of server images, so posters show instantly across
//! launches. Image URLs carry the image's tag, so a changed image gets a
//! new key and stale files simply age out.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::SystemTime;

/// Oldest files are deleted past this total size.
const MAX_BYTES: u64 = 512 * 1024 * 1024;

fn dir() -> Option<PathBuf> {
    crate::config::cache_dir().map(|dir| dir.join("images"))
}

/// Cache file for an image URL (full URL, so different servers never mix).
pub fn file_for(url: &str) -> Option<PathBuf> {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    Some(dir()?.join(format!("{:016x}", hasher.finish())))
}

pub fn read(url: &str) -> Option<Vec<u8>> {
    std::fs::read(file_for(url)?).ok()
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

/// Trims the cache to [`MAX_BYTES`], oldest first. Run once at startup.
pub fn prune() {
    let Some(dir) = dir() else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(|meta| meta.is_file())?;
            Some((meta.modified().ok()?, meta.len(), entry.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort();
    for (_, len, path) in files {
        if total <= MAX_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::file_for;

    #[test]
    fn keys_differ_per_url() {
        let a = file_for("http://a/emby/Items/1/Images/Primary?tag=x");
        let b = file_for("http://b/emby/Items/1/Images/Primary?tag=x");
        assert_ne!(a, b);
        assert_eq!(a, file_for("http://a/emby/Items/1/Images/Primary?tag=x"));
    }
}
