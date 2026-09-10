use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::Connection;

use crate::config::FrameGenBackend;
use crate::config::config_dir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemType {
    Movie,
    Series,
}

impl ItemType {
    fn as_str(self) -> &'static str {
        match self {
            ItemType::Movie => "movie",
            ItemType::Series => "series",
        }
    }

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "movie" => Ok(ItemType::Movie),
            "series" => Ok(ItemType::Series),
            other => bail!("unknown item_type {other:?} in db"),
        }
    }
}

fn frame_gen_backend_as_str(backend: FrameGenBackend) -> &'static str {
    match backend {
        FrameGenBackend::Off => "off",
        FrameGenBackend::LsfgVk => "lsfg_vk",
        FrameGenBackend::Svp => "svp",
    }
}

fn frame_gen_backend_from_str(s: &str) -> Result<FrameGenBackend> {
    match s {
        "off" => Ok(FrameGenBackend::Off),
        "lsfg_vk" => Ok(FrameGenBackend::LsfgVk),
        "svp" => Ok(FrameGenBackend::Svp),
        other => bail!("unknown frame_gen_backend {other:?} in db"),
    }
}

/// A playback preference override, keyed by `SeriesId` for episodes (so
/// changing one episode's settings applies to the whole series) or the
/// item's own `ItemId` for movies. Audio/subtitle preference is stored as
/// language+trait, never a raw track index — track layouts differ episode
/// to episode within the same series, so a stored index would silently
/// pick the wrong track on some episodes; resolve against each episode's
/// actual MediaStreams at play time instead.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleOverride {
    pub emby_item_id: String,
    pub item_type: ItemType,
    pub audio_language: Option<String>,
    pub subtitle_language: Option<String>,
    pub subtitle_forced_only: Option<bool>,
    pub frame_gen_backend: Option<FrameGenBackend>,
    pub frame_gen_multiplier: Option<u32>,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open sqlite db at {}", path.display()))?;
        Db::init_schema(&conn)?;
        Ok(Db { conn })
    }

    pub fn open_default() -> Result<Db> {
        Db::open(&default_db_path()?)
    }

    #[cfg(test)]
    fn open_in_memory() -> Result<Db> {
        let conn = Connection::open_in_memory().context("failed to open in-memory sqlite db")?;
        Db::init_schema(&conn)?;
        Ok(Db { conn })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS title_overrides (
                emby_item_id          TEXT PRIMARY KEY,
                item_type             TEXT NOT NULL,
                audio_language        TEXT,
                subtitle_language     TEXT,
                subtitle_forced_only  INTEGER,
                frame_gen_backend     TEXT,
                frame_gen_multiplier  INTEGER
            );",
        )
        .context("failed to initialize title_overrides schema")
    }

    pub fn get_override(&self, emby_item_id: &str) -> Result<Option<TitleOverride>> {
        let mut stmt = self.conn.prepare(
            "SELECT emby_item_id, item_type, audio_language, subtitle_language,
                    subtitle_forced_only, frame_gen_backend, frame_gen_multiplier
             FROM title_overrides WHERE emby_item_id = ?1",
        )?;
        let mut rows = stmt.query([emby_item_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };

        let item_type: String = row.get(1)?;
        let frame_gen_backend: Option<String> = row.get(5)?;
        let subtitle_forced_only: Option<i64> = row.get(4)?;

        Ok(Some(TitleOverride {
            emby_item_id: row.get(0)?,
            item_type: ItemType::from_str(&item_type)?,
            audio_language: row.get(2)?,
            subtitle_language: row.get(3)?,
            subtitle_forced_only: subtitle_forced_only.map(|v| v != 0),
            frame_gen_backend: frame_gen_backend
                .map(|s| frame_gen_backend_from_str(&s))
                .transpose()?,
            frame_gen_multiplier: row.get::<_, Option<i64>>(6)?.map(|v| v as u32),
        }))
    }

    // Not yet called anywhere — there's no UI to set an override yet
    // (M10, native overlay controls). `get_override` is already
    // exercised by M8's playback orchestration.
    #[allow(dead_code)]
    pub fn upsert_override(&self, entry: &TitleOverride) -> Result<()> {
        self.conn.execute(
            "INSERT INTO title_overrides
                (emby_item_id, item_type, audio_language, subtitle_language,
                 subtitle_forced_only, frame_gen_backend, frame_gen_multiplier)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(emby_item_id) DO UPDATE SET
                item_type = excluded.item_type,
                audio_language = excluded.audio_language,
                subtitle_language = excluded.subtitle_language,
                subtitle_forced_only = excluded.subtitle_forced_only,
                frame_gen_backend = excluded.frame_gen_backend,
                frame_gen_multiplier = excluded.frame_gen_multiplier",
            rusqlite::params![
                entry.emby_item_id,
                entry.item_type.as_str(),
                entry.audio_language,
                entry.subtitle_language,
                entry.subtitle_forced_only.map(|v| v as i64),
                entry.frame_gen_backend.map(frame_gen_backend_as_str),
                entry.frame_gen_multiplier.map(|v| v as i64),
            ],
        )?;
        Ok(())
    }

    #[allow(dead_code)] // same as upsert_override, above
    pub fn delete_override(&self, emby_item_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM title_overrides WHERE emby_item_id = ?1",
            [emby_item_id],
        )?;
        Ok(())
    }
}

fn default_db_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("overrides.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_series_override() {
        let db = Db::open_in_memory().unwrap();
        let entry = TitleOverride {
            emby_item_id: "series-123".to_string(),
            item_type: ItemType::Series,
            audio_language: Some("jpn".to_string()),
            subtitle_language: Some("eng".to_string()),
            subtitle_forced_only: Some(false),
            frame_gen_backend: Some(FrameGenBackend::Svp),
            frame_gen_multiplier: Some(2),
        };

        db.upsert_override(&entry).unwrap();
        let fetched = db.get_override("series-123").unwrap().unwrap();
        assert_eq!(fetched, entry);
    }

    #[test]
    fn missing_override_returns_none() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_override("does-not-exist").unwrap(), None);
    }

    #[test]
    fn upsert_overwrites_existing_row() {
        let db = Db::open_in_memory().unwrap();
        let mut entry = TitleOverride {
            emby_item_id: "movie-1".to_string(),
            item_type: ItemType::Movie,
            audio_language: None,
            subtitle_language: None,
            subtitle_forced_only: None,
            frame_gen_backend: Some(FrameGenBackend::Off),
            frame_gen_multiplier: None,
        };
        db.upsert_override(&entry).unwrap();

        entry.frame_gen_backend = Some(FrameGenBackend::LsfgVk);
        entry.frame_gen_multiplier = Some(3);
        db.upsert_override(&entry).unwrap();

        let fetched = db.get_override("movie-1").unwrap().unwrap();
        assert_eq!(fetched.frame_gen_backend, Some(FrameGenBackend::LsfgVk));
        assert_eq!(fetched.frame_gen_multiplier, Some(3));
    }

    #[test]
    fn delete_removes_the_row() {
        let db = Db::open_in_memory().unwrap();
        let entry = TitleOverride {
            emby_item_id: "movie-2".to_string(),
            item_type: ItemType::Movie,
            audio_language: None,
            subtitle_language: None,
            subtitle_forced_only: None,
            frame_gen_backend: None,
            frame_gen_multiplier: None,
        };
        db.upsert_override(&entry).unwrap();
        db.delete_override("movie-2").unwrap();
        assert_eq!(db.get_override("movie-2").unwrap(), None);
    }
}
