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
    /// mpv volume (0–130, above 100 boosts), if changed for this title.
    pub volume: Option<f64>,
}

/// How a library grid was last sorted and filtered (Emby `SortBy` and
/// `Filters` values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewPrefs {
    pub sort_by: String,
    pub descending: bool,
    pub filter: Option<String>,
    /// Further grid filters, encoded by the library page.
    pub extra: Option<String>,
}

/// The `scope` of rows saved before rows were split by server and user.
const LEGACY_SCOPE: &str = "";

const OVERRIDE_COLUMNS: &str = "emby_item_id, item_type, audio_language, subtitle_language,
    subtitle_forced_only, frame_gen_backend, frame_gen_multiplier,
    aspect_mode, zoom, shaders, volume";
const VIEW_PREFS_COLUMNS: &str = "view_key, sort_by, descending, filter";

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

    /// Brings the file to the current schema (`PRAGMA user_version`). Version
    /// 1 scopes every row by server and user; older files are rebuilt, their
    /// rows kept under [`LEGACY_SCOPE`]. Version 2 adds `tracking_queue`.
    fn init_schema(conn: &Connection) -> Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < 1 {
            Db::migrate_to_v1(conn)?;
        }
        if version < 2 {
            conn.execute_batch(
                "BEGIN;
                 CREATE TABLE IF NOT EXISTS tracking_queue (
                     id          INTEGER PRIMARY KEY AUTOINCREMENT,
                     server_id   TEXT NOT NULL,
                     item_id     TEXT NOT NULL,
                     watched     INTEGER NOT NULL,
                     services    TEXT NOT NULL,
                     created_at  INTEGER NOT NULL,
                     attempts    INTEGER NOT NULL DEFAULT 0
                 );
                 PRAGMA user_version = 2;
                 COMMIT;",
            )
            .context("failed to create tracking_queue")?;
        }
        Ok(())
    }

    fn migrate_to_v1(conn: &Connection) -> Result<()> {
        let tx = conn.unchecked_transaction()?;
        let has_table = |name: &str| -> Result<bool> {
            Ok(tx
                .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")?
                .exists([name])?)
        };
        if has_table("title_overrides")? {
            // Columns added after the first release; older databases lack them.
            for (column, kind) in [
                ("aspect_mode", "TEXT"),
                ("zoom", "REAL"),
                ("shaders", "TEXT"),
                ("volume", "REAL"),
            ] {
                let exists = tx
                    .prepare("SELECT 1 FROM pragma_table_info('title_overrides') WHERE name = ?1")?
                    .exists([column])?;
                if !exists {
                    tx.execute_batch(&format!(
                        "ALTER TABLE title_overrides ADD COLUMN {column} {kind};"
                    ))
                    .with_context(|| format!("failed to add the {column} column"))?;
                }
            }
            tx.execute_batch("ALTER TABLE title_overrides RENAME TO title_overrides_old;")?;
        }
        tx.execute_batch(
            "CREATE TABLE title_overrides (
                scope                 TEXT NOT NULL,
                emby_item_id          TEXT NOT NULL,
                item_type             TEXT NOT NULL,
                audio_language        TEXT,
                subtitle_language     TEXT,
                subtitle_forced_only  INTEGER,
                frame_gen_backend     TEXT,
                frame_gen_multiplier  INTEGER,
                aspect_mode           TEXT,
                zoom                  REAL,
                shaders               TEXT,
                volume                REAL,
                PRIMARY KEY (scope, emby_item_id)
            );",
        )
        .context("failed to initialize title_overrides schema")?;
        if has_table("title_overrides_old")? {
            tx.execute_batch(&format!(
                "INSERT INTO title_overrides (scope, {OVERRIDE_COLUMNS})
                 SELECT '{LEGACY_SCOPE}', {OVERRIDE_COLUMNS} FROM title_overrides_old;
                 DROP TABLE title_overrides_old;"
            ))
            .context("failed to move title_overrides to the scoped schema")?;
        }

        if has_table("view_prefs")? {
            tx.execute_batch("ALTER TABLE view_prefs RENAME TO view_prefs_old;")?;
        }
        tx.execute_batch(
            "CREATE TABLE view_prefs (
                scope       TEXT NOT NULL,
                view_key    TEXT NOT NULL,
                sort_by     TEXT NOT NULL,
                descending  INTEGER NOT NULL,
                filter      TEXT,
                extra       TEXT,
                PRIMARY KEY (scope, view_key)
            );",
        )
        .context("failed to initialize view_prefs schema")?;
        if has_table("view_prefs_old")? {
            tx.execute_batch(&format!(
                "INSERT INTO view_prefs (scope, {VIEW_PREFS_COLUMNS})
                 SELECT '{LEGACY_SCOPE}', {VIEW_PREFS_COLUMNS} FROM view_prefs_old;
                 DROP TABLE view_prefs_old;"
            ))
            .context("failed to move view_prefs to the scoped schema")?;
        }
        tx.execute_batch("PRAGMA user_version = 1;")?;
        tx.commit()?;
        Ok(())
    }

    /// Moves the rows saved before scoping (the only server the app could
    /// use then) to `scope`; returns how many moved.
    pub fn adopt_legacy(&self, scope: &str) -> Result<usize> {
        self.adopt_scope(LEGACY_SCOPE, scope)
    }

    /// Moves `from`'s rows to `to`; where `to` already has a row, it wins.
    pub fn adopt_scope(&self, from: &str, to: &str) -> Result<usize> {
        if from == to {
            return Ok(0);
        }
        let mut moved = 0;
        for table in ["title_overrides", "view_prefs"] {
            moved += self.conn.execute(
                &format!("UPDATE OR IGNORE {table} SET scope = ?2 WHERE scope = ?1"),
                [from, to],
            )?;
            self.conn
                .execute(&format!("DELETE FROM {table} WHERE scope = ?1"), [from])?;
        }
        Ok(moved)
    }

    pub fn get_override(&self, scope: &str, emby_item_id: &str) -> Result<Option<TitleOverride>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {OVERRIDE_COLUMNS}
             FROM title_overrides WHERE scope = ?1 AND emby_item_id = ?2"
        ))?;
        let mut rows = stmt.query([scope, emby_item_id])?;
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
            volume: row.get(10)?,
        }))
    }

    pub fn upsert_override(&self, scope: &str, entry: &TitleOverride) -> Result<()> {
        self.conn.execute(
            "INSERT INTO title_overrides
                (scope, emby_item_id, item_type, audio_language, subtitle_language,
                 subtitle_forced_only, frame_gen_backend, frame_gen_multiplier,
                 aspect_mode, zoom, shaders, volume)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(scope, emby_item_id) DO UPDATE SET
                item_type = excluded.item_type,
                audio_language = excluded.audio_language,
                subtitle_language = excluded.subtitle_language,
                subtitle_forced_only = excluded.subtitle_forced_only,
                frame_gen_backend = excluded.frame_gen_backend,
                frame_gen_multiplier = excluded.frame_gen_multiplier,
                aspect_mode = excluded.aspect_mode,
                zoom = excluded.zoom,
                shaders = excluded.shaders,
                volume = excluded.volume",
            rusqlite::params![
                scope,
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
                entry.volume,
            ],
        )?;
        Ok(())
    }

    /// A library view's remembered sort and filter, if it was changed.
    pub fn get_view_prefs(&self, scope: &str, view_key: &str) -> Result<Option<ViewPrefs>> {
        let mut statement = self.conn.prepare(
            "SELECT sort_by, descending, filter, extra FROM view_prefs
             WHERE scope = ?1 AND view_key = ?2",
        )?;
        let mut rows = statement.query([scope, view_key])?;
        Ok(match rows.next()? {
            Some(row) => Some(ViewPrefs {
                sort_by: row.get(0)?,
                descending: row.get::<_, i64>(1)? != 0,
                filter: row.get(2)?,
                extra: row.get(3)?,
            }),
            None => None,
        })
    }

    pub fn set_view_prefs(&self, scope: &str, view_key: &str, prefs: &ViewPrefs) -> Result<()> {
        self.conn.execute(
            "INSERT INTO view_prefs (scope, view_key, sort_by, descending, filter, extra)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(scope, view_key) DO UPDATE SET
                sort_by = excluded.sort_by,
                descending = excluded.descending,
                filter = excluded.filter,
                extra = excluded.extra",
            rusqlite::params![
                scope,
                view_key,
                prefs.sort_by,
                prefs.descending as i64,
                prefs.filter,
                prefs.extra
            ],
        )?;
        Ok(())
    }

    /// Forgets every remembered per-title choice, whoever's; returns how many.
    pub fn clear_overrides(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM title_overrides", [])?)
    }

    #[allow(dead_code)] // no per-title "reset" in the UI yet
    pub fn delete_override(&self, scope: &str, emby_item_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM title_overrides WHERE scope = ?1 AND emby_item_id = ?2",
            [scope, emby_item_id],
        )?;
        Ok(())
    }

    /// Queues a tracking update; `services` is a comma list of service keys.
    pub fn push_tracking(
        &self,
        server_id: &str,
        item_id: &str,
        watched: bool,
        services: &str,
        created_at: i64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO tracking_queue (server_id, item_id, watched, services, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![server_id, item_id, watched, services, created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The server's queued updates, oldest first.
    pub fn tracking_queue(&self, server_id: &str) -> Result<Vec<QueuedUpdate>> {
        let mut statement = self.conn.prepare(
            "SELECT id, server_id, item_id, watched, services, created_at, attempts
             FROM tracking_queue WHERE server_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([server_id], |row| {
            Ok(QueuedUpdate {
                id: row.get(0)?,
                server_id: row.get(1)?,
                item_id: row.get(2)?,
                watched: row.get(3)?,
                services: row.get(4)?,
                created_at: row.get(5)?,
                attempts: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn update_tracking(&self, id: i64, services: &str, attempts: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE tracking_queue SET services = ?2, attempts = ?3 WHERE id = ?1",
            rusqlite::params![id, services, attempts],
        )?;
        Ok(())
    }

    pub fn delete_tracking(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM tracking_queue WHERE id = ?1", [id])?;
        Ok(())
    }
}

/// A watched/unwatched change waiting to reach the tracking services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedUpdate {
    pub id: i64,
    pub server_id: String,
    pub item_id: String,
    pub watched: bool,
    pub services: String,
    pub created_at: i64,
    pub attempts: i64,
}

fn default_db_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("overrides.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "srv|user";

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
            volume: None,
        };

        db.upsert_override(S, &entry).unwrap();
        let fetched = db.get_override(S, "series-123").unwrap().unwrap();
        assert_eq!(fetched, entry);
    }

    #[test]
    fn missing_override_returns_none() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_override(S, "does-not-exist").unwrap(), None);
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
            volume: None,
        };
        db.upsert_override(S, &entry).unwrap();

        entry.frame_gen_backend = Some(FrameGenBackend::Svp);
        entry.frame_gen_multiplier = Some(3);
        db.upsert_override(S, &entry).unwrap();

        let fetched = db.get_override(S, "movie-1").unwrap().unwrap();
        assert_eq!(fetched.frame_gen_backend, Some(FrameGenBackend::Svp));
        assert_eq!(fetched.frame_gen_multiplier, Some(3));
    }

    #[test]
    fn legacy_lsfg_row_reads_as_off() {
        let db = Db::open_in_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO title_overrides (scope, emby_item_id, item_type, frame_gen_backend)
                 VALUES ('srv|user', 'series-legacy', 'series', 'lsfg_vk')",
                [],
            )
            .unwrap();
        let fetched = db.get_override(S, "series-legacy").unwrap().unwrap();
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
            volume: None,
        };
        db.upsert_override(S, &entry).unwrap();
        db.delete_override(S, "movie-2").unwrap();
        assert_eq!(db.get_override(S, "movie-2").unwrap(), None);
    }

    #[test]
    fn clear_overrides_removes_everything() {
        let db = Db::open(
            &std::env::temp_dir().join(format!("embyclientplus-clear-{}.db", std::process::id())),
        )
        .unwrap();
        for id in ["a", "b"] {
            db.upsert_override(
                S,
                &TitleOverride {
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
                    volume: None,
                },
            )
            .unwrap();
        }
        assert_eq!(db.clear_overrides().unwrap(), 2);
        assert!(db.get_override(S, "a").unwrap().is_none());
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
        let old = db.get_override("", "old").unwrap().unwrap();
        assert_eq!(old.audio_language.as_deref(), Some("jpn"));
        assert_eq!(old.aspect_mode, None);
        let mut changed = old.clone();
        changed.aspect_mode = Some("fill".into());
        changed.zoom = Some(0.2);
        db.upsert_override("", &changed).unwrap();
        assert_eq!(db.get_override("", "old").unwrap().unwrap().zoom, Some(0.2));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn view_prefs_round_trip_and_update() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_view_prefs(S, "lib|Movie").unwrap(), None);
        let prefs = ViewPrefs {
            sort_by: "DateCreated".into(),
            descending: true,
            filter: Some("IsUnplayed".into()),
            extra: Some("Genres=Drama".into()),
        };
        db.set_view_prefs(S, "lib|Movie", &prefs).unwrap();
        assert_eq!(db.get_view_prefs(S, "lib|Movie").unwrap(), Some(prefs));
        let changed = ViewPrefs {
            sort_by: "SortName".into(),
            descending: false,
            filter: None,
            extra: None,
        };
        db.set_view_prefs(S, "lib|Movie", &changed).unwrap();
        assert_eq!(db.get_view_prefs(S, "lib|Movie").unwrap(), Some(changed));
    }

    fn movie(id: &str, audio: &str) -> TitleOverride {
        TitleOverride {
            emby_item_id: id.into(),
            item_type: ItemType::Movie,
            audio_language: Some(audio.into()),
            subtitle_language: None,
            subtitle_forced_only: None,
            frame_gen_backend: None,
            frame_gen_multiplier: None,
            aspect_mode: None,
            zoom: None,
            shaders: None,
            volume: None,
        }
    }

    fn prefs(sort_by: &str) -> ViewPrefs {
        ViewPrefs {
            sort_by: sort_by.into(),
            descending: false,
            filter: None,
            extra: None,
        }
    }

    fn user_version(db: &Db) -> i64 {
        db.conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn scopes_do_not_see_each_other() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_override("a|u1", &movie("m", "jpn")).unwrap();
        db.set_view_prefs("a|u1", "lib|Movie", &prefs("SortName"))
            .unwrap();
        for other in ["a|u2", "b|u1", "local"] {
            assert_eq!(db.get_override(other, "m").unwrap(), None);
            assert_eq!(db.get_view_prefs(other, "lib|Movie").unwrap(), None);
        }
        db.upsert_override("b|u1", &movie("m", "eng")).unwrap();
        let mine = db.get_override("a|u1", "m").unwrap().unwrap();
        assert_eq!(mine.audio_language.as_deref(), Some("jpn"));
        db.delete_override("b|u1", "m").unwrap();
        assert!(db.get_override("a|u1", "m").unwrap().is_some());
    }

    #[test]
    fn view_prefs_extra_round_trips() {
        let db = Db::open_in_memory().unwrap();
        let mut with_extra = prefs("SortName");
        with_extra.extra = Some("Genres=Drama".into());
        db.set_view_prefs(S, "k", &with_extra).unwrap();
        assert_eq!(db.get_view_prefs(S, "k").unwrap(), Some(with_extra));
    }

    /// The schema as of 0.2.x, before scoping and the later columns.
    fn old_database(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE title_overrides (emby_item_id TEXT PRIMARY KEY, item_type TEXT NOT NULL,
             audio_language TEXT, subtitle_language TEXT, subtitle_forced_only INTEGER,
             frame_gen_backend TEXT, frame_gen_multiplier INTEGER, aspect_mode TEXT,
             zoom REAL, shaders TEXT, volume REAL);
             INSERT INTO title_overrides (emby_item_id, item_type, audio_language, volume)
             VALUES ('old', 'movie', 'jpn', 80.0);
             CREATE TABLE view_prefs (view_key TEXT PRIMARY KEY, sort_by TEXT NOT NULL,
             descending INTEGER NOT NULL, filter TEXT);
             INSERT INTO view_prefs VALUES ('lib|Movie', 'DateCreated', 1, 'IsUnplayed');",
        )
        .unwrap();
    }

    #[test]
    fn unscoped_databases_upgrade_and_are_adopted() {
        let path =
            std::env::temp_dir().join(format!("embyclientplus-scope-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        old_database(&path);
        let db = Db::open(&path).unwrap();
        assert_eq!(user_version(&db), 2);
        let old = db.get_override("", "old").unwrap().unwrap();
        assert_eq!(old.audio_language.as_deref(), Some("jpn"));
        assert_eq!(old.volume, Some(80.0));
        let view = db.get_view_prefs("", "lib|Movie").unwrap().unwrap();
        assert_eq!(view.sort_by, "DateCreated");
        assert!(view.descending && view.extra.is_none());
        // Nothing leaks into a real scope until it adopts them.
        assert_eq!(db.get_override(S, "old").unwrap(), None);

        assert_eq!(db.adopt_legacy(S).unwrap(), 2);
        assert!(db.get_override(S, "old").unwrap().is_some());
        assert!(db.get_view_prefs(S, "lib|Movie").unwrap().is_some());
        assert_eq!(db.get_override("", "old").unwrap(), None);
        assert_eq!(db.adopt_legacy("other|user").unwrap(), 0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn adopting_keeps_rows_the_scope_already_has() {
        let db = Db::open_in_memory().unwrap();
        db.upsert_override("", &movie("m", "legacy")).unwrap();
        db.upsert_override("", &movie("n", "legacy")).unwrap();
        db.upsert_override(S, &movie("m", "mine")).unwrap();
        db.adopt_legacy(S).unwrap();
        let kept = db.get_override(S, "m").unwrap().unwrap();
        assert_eq!(kept.audio_language.as_deref(), Some("mine"));
        assert!(db.get_override(S, "n").unwrap().is_some());
        assert_eq!(db.get_override("", "m").unwrap(), None);
    }

    #[test]
    fn upgrading_twice_changes_nothing() {
        let path =
            std::env::temp_dir().join(format!("embyclientplus-twice-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        old_database(&path);
        drop(Db::open(&path).unwrap());
        let db = Db::open(&path).unwrap();
        assert_eq!(user_version(&db), 2);
        assert!(db.get_override("", "old").unwrap().is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tracking_queue_keeps_order_and_scopes_by_server() {
        let db = Db::open_in_memory().unwrap();
        db.push_tracking("srv-a", "item1", true, "mal,trakt", 10)
            .unwrap();
        db.push_tracking("srv-b", "item2", true, "mal", 11).unwrap();
        db.push_tracking("srv-a", "item1", false, "mal", 12)
            .unwrap();
        let rows = db.tracking_queue("srv-a").unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].watched);
        assert!(!rows[1].watched);
        db.update_tracking(rows[0].id, "trakt", 1).unwrap();
        let rows = db.tracking_queue("srv-a").unwrap();
        assert_eq!(rows[0].services, "trakt");
        assert_eq!(rows[0].attempts, 1);
        db.delete_tracking(rows[0].id).unwrap();
        assert_eq!(db.tracking_queue("srv-a").unwrap().len(), 1);
    }
}
