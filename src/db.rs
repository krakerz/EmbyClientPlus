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
        FrameGenBackend::Svp => "svp",
    }
}

fn frame_gen_backend_from_str(s: &str) -> Result<FrameGenBackend> {
    match s {
        // lsfg-vk was a backend before 0.2.0; existing rows fall back to off.
        "off" | "lsfg_vk" => Ok(FrameGenBackend::Off),
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
    /// Picture aspect mode name (see `player::Aspect`), if changed.
    pub aspect_mode: Option<String>,
    /// Extra zoom (log2 steps) on top of the automatic fit.
    pub zoom: Option<f64>,
    /// Shader preset per group (see `shaders::encode_choices`).
    pub shaders: Option<String>,
}

/// How a library grid was last sorted and filtered (Emby `SortBy` and
/// `Filters` values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewPrefs {
    pub sort_by: String,
    pub descending: bool,
    pub filter: Option<String>,
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
        .context("failed to initialize title_overrides schema")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS view_prefs (
                view_key    TEXT PRIMARY KEY,
                sort_by     TEXT NOT NULL,
                descending  INTEGER NOT NULL,
                filter      TEXT
            );",
        )
        .context("failed to initialize view_prefs schema")?;
        // Columns added after the first release; older databases lack them.
        for (column, kind) in [
            ("aspect_mode", "TEXT"),
            ("zoom", "REAL"),
            ("shaders", "TEXT"),
        ] {
            let exists = conn
                .prepare("SELECT 1 FROM pragma_table_info('title_overrides') WHERE name = ?1")?
                .exists([column])?;
            if !exists {
                conn.execute_batch(&format!(
                    "ALTER TABLE title_overrides ADD COLUMN {column} {kind};"
                ))
                .with_context(|| format!("failed to add the {column} column"))?;
            }
        }
        Ok(())
    }

    pub fn get_override(&self, emby_item_id: &str) -> Result<Option<TitleOverride>> {
        let mut stmt = self.conn.prepare(
            "SELECT emby_item_id, item_type, audio_language, subtitle_language,
                    subtitle_forced_only, frame_gen_backend, frame_gen_multiplier,
                    aspect_mode, zoom, shaders
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
            aspect_mode: row.get(7)?,
            zoom: row.get(8)?,
            shaders: row.get(9)?,
        }))
    }

    pub fn upsert_override(&self, entry: &TitleOverride) -> Result<()> {
        self.conn.execute(
            "INSERT INTO title_overrides
                (emby_item_id, item_type, audio_language, subtitle_language,
                 subtitle_forced_only, frame_gen_backend, frame_gen_multiplier,
                 aspect_mode, zoom, shaders)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(emby_item_id) DO UPDATE SET
                item_type = excluded.item_type,
                audio_language = excluded.audio_language,
                subtitle_language = excluded.subtitle_language,
                subtitle_forced_only = excluded.subtitle_forced_only,
                frame_gen_backend = excluded.frame_gen_backend,
                frame_gen_multiplier = excluded.frame_gen_multiplier,
                aspect_mode = excluded.aspect_mode,
                zoom = excluded.zoom,
                shaders = excluded.shaders",
            rusqlite::params![
                entry.emby_item_id,
                entry.item_type.as_str(),
                entry.audio_language,
                entry.subtitle_language,
                entry.subtitle_forced_only.map(|v| v as i64),
                entry.frame_gen_backend.map(frame_gen_backend_as_str),
                entry.frame_gen_multiplier.map(|v| v as i64),
                entry.aspect_mode,
                entry.zoom,
                entry.shaders,
            ],
        )?;
        Ok(())
    }

    /// A library view's remembered sort and filter, if it was changed.
    pub fn get_view_prefs(&self, view_key: &str) -> Result<Option<ViewPrefs>> {
        let mut statement = self
            .conn
            .prepare("SELECT sort_by, descending, filter FROM view_prefs WHERE view_key = ?1")?;
        let mut rows = statement.query([view_key])?;
        Ok(match rows.next()? {
            Some(row) => Some(ViewPrefs {
                sort_by: row.get(0)?,
                descending: row.get::<_, i64>(1)? != 0,
                filter: row.get(2)?,
            }),
            None => None,
        })
    }

    pub fn set_view_prefs(&self, view_key: &str, prefs: &ViewPrefs) -> Result<()> {
        self.conn.execute(
            "INSERT INTO view_prefs (view_key, sort_by, descending, filter)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(view_key) DO UPDATE SET
                sort_by = excluded.sort_by,
                descending = excluded.descending,
                filter = excluded.filter",
            rusqlite::params![
                view_key,
                prefs.sort_by,
                prefs.descending as i64,
                prefs.filter
            ],
        )?;
        Ok(())
    }

    /// Forgets every remembered per-title choice; returns how many.
    pub fn clear_overrides(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM title_overrides", [])?)
    }

    #[allow(dead_code)] // no per-title "reset" in the UI yet
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
            aspect_mode: None,
            zoom: None,
            shaders: None,
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
            aspect_mode: None,
            zoom: None,
            shaders: None,
        };
        db.upsert_override(&entry).unwrap();

        entry.frame_gen_backend = Some(FrameGenBackend::Svp);
        entry.frame_gen_multiplier = Some(3);
        db.upsert_override(&entry).unwrap();

        let fetched = db.get_override("movie-1").unwrap().unwrap();
        assert_eq!(fetched.frame_gen_backend, Some(FrameGenBackend::Svp));
        assert_eq!(fetched.frame_gen_multiplier, Some(3));
    }

    #[test]
    fn legacy_lsfg_row_reads_as_off() {
        let db = Db::open_in_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO title_overrides (emby_item_id, item_type, frame_gen_backend)
                 VALUES ('series-legacy', 'series', 'lsfg_vk')",
                [],
            )
            .unwrap();
        let fetched = db.get_override("series-legacy").unwrap().unwrap();
        assert_eq!(fetched.frame_gen_backend, Some(FrameGenBackend::Off));
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
            aspect_mode: None,
            zoom: None,
            shaders: None,
        };
        db.upsert_override(&entry).unwrap();
        db.delete_override("movie-2").unwrap();
        assert_eq!(db.get_override("movie-2").unwrap(), None);
    }

    #[test]
    fn clear_overrides_removes_everything() {
        let db = Db::open(
            &std::env::temp_dir().join(format!("embyclientplus-clear-{}.db", std::process::id())),
        )
        .unwrap();
        for id in ["a", "b"] {
            db.upsert_override(&TitleOverride {
                emby_item_id: id.into(),
                item_type: ItemType::Movie,
                audio_language: Some("jpn".into()),
                subtitle_language: None,
                subtitle_forced_only: None,
                frame_gen_backend: None,
                frame_gen_multiplier: None,
                aspect_mode: None,
                zoom: None,
                shaders: None,
            })
            .unwrap();
        }
        assert_eq!(db.clear_overrides().unwrap(), 2);
        assert!(db.get_override("a").unwrap().is_none());
    }

    #[test]
    fn old_databases_gain_the_new_columns() {
        let path =
            std::env::temp_dir().join(format!("embyclientplus-migrate-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE title_overrides (emby_item_id TEXT PRIMARY KEY, item_type TEXT NOT NULL,
                 audio_language TEXT, subtitle_language TEXT, subtitle_forced_only INTEGER,
                 frame_gen_backend TEXT, frame_gen_multiplier INTEGER);
                 INSERT INTO title_overrides (emby_item_id, item_type, audio_language)
                 VALUES ('old', 'movie', 'jpn');",
            )
            .unwrap();
        }
        let db = Db::open(&path).unwrap();
        let old = db.get_override("old").unwrap().unwrap();
        assert_eq!(old.audio_language.as_deref(), Some("jpn"));
        assert_eq!(old.aspect_mode, None);
        let mut changed = old.clone();
        changed.aspect_mode = Some("fill".into());
        changed.zoom = Some(0.2);
        db.upsert_override(&changed).unwrap();
        assert_eq!(db.get_override("old").unwrap().unwrap().zoom, Some(0.2));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn view_prefs_round_trip_and_update() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_view_prefs("lib|Movie").unwrap(), None);
        let prefs = ViewPrefs {
            sort_by: "DateCreated".into(),
            descending: true,
            filter: Some("IsUnplayed".into()),
        };
        db.set_view_prefs("lib|Movie", &prefs).unwrap();
        assert_eq!(db.get_view_prefs("lib|Movie").unwrap(), Some(prefs));
        let changed = ViewPrefs {
            sort_by: "SortName".into(),
            descending: false,
            filter: None,
        };
        db.set_view_prefs("lib|Movie", &changed).unwrap();
        assert_eq!(db.get_view_prefs("lib|Movie").unwrap(), Some(changed));
    }
}
