//! What Trakt told us, kept in one JSON file in the cache folder so a restart
//! doesn't repeat the calls (the free API quota is small). It belongs to the
//! logged-in account and is deleted with the login.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const FILE_NAME: &str = "trakt-cache.json";

/// The persisted data, borrowed for writing (same shape as [`Disk`]).
#[derive(Debug, Serialize)]
pub(super) struct Snapshot<'a, E> {
    pub activity: Option<&'a str>,
    pub shows: Option<&'a Value>,
    pub show_ids: &'a HashMap<String, Option<u64>>,
    pub numbering: HashMap<u64, &'a E>,
}

/// The persisted data. `E` is the per-show episode numbering.
#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "E: Deserialize<'de>"))]
pub(super) struct Disk<E> {
    /// `last_activities` fingerprint the watched shows were read at.
    #[serde(default)]
    pub activity: Option<String>,
    /// The raw `/sync/watched/shows` answer.
    #[serde(default)]
    pub shows: Option<Value>,
    /// Show key → Trakt id (`None`: Trakt doesn't know the show).
    #[serde(default)]
    pub show_ids: HashMap<String, Option<u64>>,
    /// Trakt show id → numbering.
    #[serde(default)]
    pub numbering: HashMap<u64, E>,
}

impl<E> Default for Disk<E> {
    fn default() -> Self {
        Disk {
            activity: None,
            shows: None,
            show_ids: HashMap::new(),
            numbering: HashMap::new(),
        }
    }
}

/// The cache file in the app's cache folder.
pub(super) fn path() -> Option<PathBuf> {
    crate::config::cache_dir().map(|dir| dir.join(FILE_NAME))
}

/// Reads the file; a missing or unreadable one is an empty cache.
pub(super) fn load<E: DeserializeOwned>(path: &Path) -> Disk<E> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("Trakt cache unreadable: {e}");
            }
            return Disk::default();
        }
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        tracing::warn!("Trakt cache ignored: {e}");
        Disk::default()
    })
}

/// Writes the file atomically, creating the folder.
pub(super) fn save<E: Serialize>(path: &Path, disk: &Snapshot<'_, E>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec(disk).map_err(std::io::Error::other)?;
    crate::config::write_atomic(path, &bytes)
}

/// Removes the cache file (it is the old account's).
pub(super) fn delete() {
    let Some(path) = path() else {
        return;
    };
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("Trakt cache not deleted: {e}");
    }
}

/// The parts of a `/sync/last_activities` answer that change the watched
/// shows: episode plays, hidden and reset shows or seasons. `None` without
/// the episode play time (never trust such an answer).
pub(super) fn fingerprint(body: &Value) -> Option<String> {
    let watched = body.pointer("/episodes/watched_at")?.as_str()?;
    let mut parts = vec![watched.to_string()];
    for pointer in [
        "/shows/hidden_at",
        "/shows/reset_at",
        "/seasons/hidden_at",
        "/seasons/reset_at",
        "/episodes/reset_at",
    ] {
        parts.push(
            body.pointer(pointer)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }
    Some(parts.join("|"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_file(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ecp-trakt-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("nested").join(FILE_NAME)
    }

    #[test]
    fn round_trip() {
        let path = temp_file("round");
        let disk: Disk<Vec<u32>> = Disk {
            activity: Some("a|b".to_string()),
            shows: Some(json!([{"show": {"ids": {"trakt": 1}}}])),
            show_ids: HashMap::from([
                ("tvdb:1".to_string(), Some(5)),
                ("tvdb:2".to_string(), None),
            ]),
            numbering: HashMap::from([(5, vec![1, 2])]),
        };
        let snapshot = Snapshot {
            activity: disk.activity.as_deref(),
            shows: disk.shows.as_ref(),
            show_ids: &disk.show_ids,
            numbering: disk.numbering.iter().map(|(k, v)| (*k, v)).collect(),
        };
        save(&path, &snapshot).unwrap();
        let back: Disk<Vec<u32>> = load(&path);
        assert_eq!(back.activity, disk.activity);
        assert_eq!(back.shows, disk.shows);
        assert_eq!(back.show_ids, disk.show_ids);
        assert_eq!(back.numbering, disk.numbering);
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn missing_or_broken_file_is_empty() {
        let path = temp_file("broken");
        let none: Disk<u32> = load(&path);
        assert!(none.shows.is_none() && none.show_ids.is_empty());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not json").unwrap();
        let broken: Disk<u32> = load(&path);
        assert!(broken.activity.is_none() && broken.numbering.is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn fingerprint_compares_the_watched_fields() {
        let base = json!({
            "all": "2024-01-01T00:00:00.000Z",
            "episodes": {"watched_at": "2024-01-01T00:00:00.000Z", "rated_at": "x"},
            "shows": {"hidden_at": "2023-01-01T00:00:00.000Z"},
        });
        let same_but_rating = json!({
            "all": "2025-01-01T00:00:00.000Z",
            "episodes": {"watched_at": "2024-01-01T00:00:00.000Z", "rated_at": "y"},
            "shows": {"hidden_at": "2023-01-01T00:00:00.000Z"},
        });
        let watched = json!({
            "episodes": {"watched_at": "2024-02-01T00:00:00.000Z"},
            "shows": {"hidden_at": "2023-01-01T00:00:00.000Z"},
        });
        let hidden = json!({
            "episodes": {"watched_at": "2024-01-01T00:00:00.000Z"},
            "shows": {"hidden_at": "2024-03-01T00:00:00.000Z"},
        });
        let fp = fingerprint(&base);
        assert!(fp.is_some());
        assert_eq!(fp, fingerprint(&same_but_rating));
        assert_ne!(fp, fingerprint(&watched));
        assert_ne!(fp, fingerprint(&hidden));
        assert_eq!(fingerprint(&json!({"episodes": {}})), None);
        assert_eq!(fingerprint(&Value::Null), None);
    }
}
