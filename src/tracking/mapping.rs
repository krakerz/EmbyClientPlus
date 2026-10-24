//! The AniBridge mappings (<https://github.com/anibridge/anibridge-mappings>):
//! which MAL / AniList entry an episode of a TVDB / TMDB / AniDB show-season
//! belongs to, and at which episode number there. The file is keyed by every
//! descriptor and already holds the reverse edges, so the index is the file
//! as-is. Downloaded to the cache dir and refreshed weekly.

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};

const URL: &str =
    "https://github.com/anibridge/anibridge-mappings/releases/download/v3/mappings.min.json";
const FILE_NAME: &str = "mappings.min.json";
const ETAG_NAME: &str = "mappings.etag";
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AniDbScope {
    Regular,
    Special,
    Other,
    Credits,
    Trailers,
    Parodies,
}

impl AniDbScope {
    fn letter(self) -> char {
        match self {
            AniDbScope::Regular => 'R',
            AniDbScope::Special => 'S',
            AniDbScope::Other => 'O',
            AniDbScope::Credits => 'C',
            AniDbScope::Trailers => 'T',
            AniDbScope::Parodies => 'P',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Descriptor {
    AniDb { id: u32, scope: AniDbScope },
    Mal(u32),
    AniList(u32),
    TvdbShow { id: u32, season: u32 },
    TmdbShow { id: u32, season: u32 },
    ImdbShow { id: Box<str>, season: u32 },
    TmdbMovie(u32),
    TvdbMovie(u32),
    ImdbMovie(Box<str>),
}

impl Descriptor {
    pub fn parse(s: &str) -> Option<Descriptor> {
        let mut parts = s.split(':');
        let provider = parts.next()?;
        let id = parts.next()?;
        let scope = parts.next();
        if parts.next().is_some() {
            return None;
        }
        let number = || id.parse::<u32>().ok();
        let season = || scope?.strip_prefix('s')?.parse::<u32>().ok();
        Some(match (provider, scope) {
            ("anidb", Some(scope)) => Descriptor::AniDb {
                id: number()?,
                scope: match scope {
                    "R" => AniDbScope::Regular,
                    "S" => AniDbScope::Special,
                    "O" => AniDbScope::Other,
                    "C" => AniDbScope::Credits,
                    "T" => AniDbScope::Trailers,
                    "P" => AniDbScope::Parodies,
                    _ => return None,
                },
            },
            ("mal", None) => Descriptor::Mal(number()?),
            ("anilist", None) => Descriptor::AniList(number()?),
            ("tvdb_show", Some(_)) => Descriptor::TvdbShow {
                id: number()?,
                season: season()?,
            },
            ("tmdb_show", Some(_)) => Descriptor::TmdbShow {
                id: number()?,
                season: season()?,
            },
            ("imdb_show", Some(_)) => Descriptor::ImdbShow {
                id: id.into(),
                season: season()?,
            },
            ("tmdb_movie", None) => Descriptor::TmdbMovie(number()?),
            ("tvdb_movie", None) => Descriptor::TvdbMovie(number()?),
            ("imdb_movie", None) => Descriptor::ImdbMovie(id.into()),
            _ => return None,
        })
    }

    /// Kinds the resolver follows; other targets are dropped at load.
    fn is_resolver_target(&self) -> bool {
        matches!(
            self,
            Descriptor::Mal(_)
                | Descriptor::AniList(_)
                | Descriptor::AniDb { .. }
                | Descriptor::TvdbShow { .. }
                | Descriptor::TmdbShow { .. }
        )
    }
}

impl fmt::Display for Descriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Descriptor::AniDb { id, scope } => write!(f, "anidb:{id}:{}", scope.letter()),
            Descriptor::Mal(id) => write!(f, "mal:{id}"),
            Descriptor::AniList(id) => write!(f, "anilist:{id}"),
            Descriptor::TvdbShow { id, season } => write!(f, "tvdb_show:{id}:s{season}"),
            Descriptor::TmdbShow { id, season } => write!(f, "tmdb_show:{id}:s{season}"),
            Descriptor::ImdbShow { id, season } => write!(f, "imdb_show:{id}:s{season}"),
            Descriptor::TmdbMovie(id) => write!(f, "tmdb_movie:{id}"),
            Descriptor::TvdbMovie(id) => write!(f, "tvdb_movie:{id}"),
            Descriptor::ImdbMovie(id) => write!(f, "imdb_movie:{id}"),
        }
    }
}

/// Inclusive episode span; `end: None` is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: Option<u32>,
}

impl Span {
    pub fn contains(&self, episode: u32) -> bool {
        episode >= self.start && self.end.is_none_or(|end| episode <= end)
    }
}

fn parse_span(s: &str) -> Option<Span> {
    let span = match s.split_once('-') {
        None => {
            let episode = s.parse().ok()?;
            Span {
                start: episode,
                end: Some(episode),
            }
        }
        Some((start, "")) => Span {
            start: start.parse().ok()?,
            end: None,
        },
        Some((start, end)) => Span {
            start: start.parse().ok()?,
            end: Some(end.parse().ok()?),
        },
    };
    match span.end {
        Some(end) if end < span.start => None,
        _ => Some(span),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeMap {
    pub source: Span,
    pub target: Box<[Span]>,
    /// 0 = none.
    pub ratio: i32,
}

impl RangeMap {
    pub fn parse(source: &str, target: &str) -> Option<RangeMap> {
        let (segments, ratio) = match target.split_once('|') {
            Some((segments, ratio)) => (segments, ratio.parse::<i32>().ok().filter(|r| *r != 0)?),
            None => (target, 0),
        };
        let target: Box<[Span]> = segments.split(',').map(parse_span).collect::<Option<_>>()?;
        // Only the last segment may be open.
        if target.iter().rev().skip(1).any(|span| span.end.is_none()) {
            return None;
        }
        Some(RangeMap {
            source: parse_span(source)?,
            target,
            ratio,
        })
    }

    /// The target episode that watching `source_episode` completes, if any.
    pub fn map(&self, source_episode: u32) -> Option<u32> {
        if !self.source.contains(source_episode) {
            return None;
        }
        let offset = source_episode - self.source.start;
        // Ratio sign as the data uses it (the README states it reversed):
        // -n: one source episode spans n target episodes; n: n source
        // episodes make one target episode.
        let index = match self.ratio {
            0 => offset,
            r if r < 0 => (offset + 1).checked_mul(r.unsigned_abs())? - 1,
            r => ((offset + 1) / r as u32).checked_sub(1)?,
        };
        nth_episode(&self.target, index)
    }
}

fn nth_episode(segments: &[Span], mut index: u32) -> Option<u32> {
    for segment in segments {
        match segment.end {
            None => return segment.start.checked_add(index),
            Some(end) => {
                let len = end - segment.start + 1;
                if index < len {
                    return Some(segment.start + index);
                }
                index -= len;
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub target: Descriptor,
    pub ranges: Box<[RangeMap]>,
}

pub struct AniBridge {
    entries: HashMap<Descriptor, Box<[Edge]>>,
    generated_on: Option<String>,
}

impl AniBridge {
    pub fn from_slice(bytes: &[u8]) -> Result<AniBridge> {
        let file = serde_json::from_slice::<RawFile>(bytes)
            .context("AniBridge mappings aren't valid JSON")?;
        match file.schema_version.as_deref() {
            None => bail!("AniBridge mappings have no $meta"),
            Some(version) if !version.starts_with("3.") => {
                bail!("unsupported AniBridge schema {version}")
            }
            Some(_) => {}
        }
        tracing::debug!("AniBridge: {} entries", file.entries.len());
        Ok(AniBridge {
            entries: file.entries,
            generated_on: file.generated_on,
        })
    }

    /// Empty slice when unlisted.
    pub fn edges(&self, key: &Descriptor) -> &[Edge] {
        self.entries.get(key).map_or(&[], |edges| edges)
    }

    pub fn contains(&self, key: &Descriptor) -> bool {
        self.entries.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn generated_on(&self) -> Option<&str> {
        self.generated_on.as_deref()
    }
}

#[derive(Default)]
struct RawFile {
    entries: HashMap<Descriptor, Box<[Edge]>>,
    schema_version: Option<String>,
    generated_on: Option<String>,
}

#[derive(serde::Deserialize)]
struct Meta {
    schema_version: String,
    generated_on: Option<String>,
}

impl<'de> serde::Deserialize<'de> for RawFile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FileVisitor;
        impl<'de> serde::de::Visitor<'de> for FileVisitor {
            type Value = RawFile;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an AniBridge mappings object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<RawFile, A::Error> {
                let mut file = RawFile::default();
                while let Some(key) = map.next_key::<String>()? {
                    if key == "$meta" {
                        let meta: Meta = map.next_value()?;
                        file.schema_version = Some(meta.schema_version);
                        file.generated_on = meta.generated_on;
                        continue;
                    }
                    let targets: HashMap<String, HashMap<String, String>> = map.next_value()?;
                    let Some(source) = Descriptor::parse(&key) else {
                        continue;
                    };
                    let edges: Box<[Edge]> = targets
                        .iter()
                        .filter_map(|(target, ranges)| {
                            let target =
                                Descriptor::parse(target).filter(Descriptor::is_resolver_target)?;
                            let ranges: Box<[RangeMap]> = ranges
                                .iter()
                                .filter_map(|(source, target)| RangeMap::parse(source, target))
                                .collect();
                            (!ranges.is_empty()).then_some(Edge { target, ranges })
                        })
                        .collect();
                    if !edges.is_empty() {
                        file.entries.insert(source, edges);
                    }
                }
                Ok(file)
            }
        }
        deserializer.deserialize_map(FileVisitor)
    }
}

struct Loaded {
    index: Arc<AniBridge>,
    next_check: Instant,
}

static LOADED: tokio::sync::Mutex<Option<Loaded>> = tokio::sync::Mutex::const_new(None);

enum Fetched {
    NotModified,
    New(Arc<AniBridge>),
}

/// The process-wide index: loads the cached file, refreshing it from GitHub
/// when it's older than 7 days. Call on tokio (it does blocking work in
/// `spawn_blocking`).
pub async fn index() -> Result<Arc<AniBridge>> {
    let mut loaded = LOADED.lock().await;
    if let Some(current) = loaded.as_ref()
        && Instant::now() < current.next_check
    {
        return Ok(current.index.clone());
    }
    let dir = crate::config::cache_dir()
        .context("no cache directory")?
        .join("anibridge");
    let file = dir.join(FILE_NAME);
    let etag_file = dir.join(ETAG_NAME);
    let fresh = std::fs::metadata(&file)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age < MAX_AGE);
    let mut downloaded = None;
    let next_check = if fresh {
        Instant::now() + MAX_AGE
    } else {
        match download(&file, &etag_file).await {
            Ok(Fetched::NotModified) => {
                let touched = std::fs::File::options()
                    .write(true)
                    .open(&file)
                    .and_then(|f| f.set_modified(SystemTime::now()));
                if let Err(e) = touched {
                    tracing::warn!("couldn't refresh the AniBridge file's age: {e}");
                }
                Instant::now() + MAX_AGE
            }
            Ok(Fetched::New(index)) => {
                tracing::info!(
                    "AniBridge mappings updated: {} entries, generated {}",
                    index.len(),
                    index.generated_on().unwrap_or("at an unknown time")
                );
                downloaded = Some(index);
                Instant::now() + MAX_AGE
            }
            Err(e) => {
                tracing::warn!("AniBridge download failed, keeping the old copy: {e:#}");
                Instant::now() + RETRY_AFTER
            }
        }
    };
    let index = match downloaded.or_else(|| loaded.as_ref().map(|l| l.index.clone())) {
        Some(index) => index,
        None => {
            let path = file.clone();
            tokio::task::spawn_blocking(move || -> Result<Arc<AniBridge>> {
                let bytes =
                    std::fs::read(&path).context("AniBridge mappings aren't downloaded yet")?;
                Ok(Arc::new(AniBridge::from_slice(&bytes)?))
            })
            .await
            .context("the AniBridge loader panicked")??
        }
    };
    *loaded = Some(Loaded {
        index: index.clone(),
        next_check,
    });
    Ok(index)
}

async fn download(file: &Path, etag_file: &Path) -> Result<Fetched> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("EmbyClientPlus/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(120))
        .build()
        .context("couldn't create the HTTP client")?;
    let mut request = client.get(URL);
    if file.exists()
        && let Ok(etag) = std::fs::read_to_string(etag_file)
    {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag.trim());
    }
    let response = request.send().await.context("couldn't reach GitHub")?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Fetched::NotModified);
    }
    let response = response
        .error_for_status()
        .context("GitHub refused the download")?;
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response.bytes().await.context("the download was cut off")?;
    let (file, etag_file) = (file.to_path_buf(), etag_file.to_path_buf());
    tokio::task::spawn_blocking(move || -> Result<Fetched> {
        // Parse first: a bad download must not replace a working copy.
        let index = AniBridge::from_slice(&bytes)?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).context("couldn't create the cache folder")?;
        }
        let part = file.with_extension("json.part");
        std::fs::write(&part, &bytes).context("couldn't write the mappings file")?;
        std::fs::rename(&part, &file).context("couldn't replace the mappings file")?;
        match etag {
            Some(etag) => std::fs::write(&etag_file, etag).context("couldn't write the etag")?,
            None => {
                let _ = std::fs::remove_file(&etag_file);
            }
        }
        Ok(Fetched::New(Arc::new(index)))
    })
    .await
    .context("the AniBridge writer panicked")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_parse_and_round_trip() {
        let cases = [
            (
                "anidb:19248:R",
                Descriptor::AniDb {
                    id: 19248,
                    scope: AniDbScope::Regular,
                },
            ),
            (
                "tvdb_show:355774:s4",
                Descriptor::TvdbShow {
                    id: 355774,
                    season: 4,
                },
            ),
            (
                "imdb_movie:tt0275277",
                Descriptor::ImdbMovie("tt0275277".into()),
            ),
            ("mal:1", Descriptor::Mal(1)),
        ];
        for (text, expected) in cases {
            let parsed = Descriptor::parse(text).unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(parsed.to_string(), text);
        }
        for bad in [
            "kitsu:1",
            "mal:1:x",
            "tvdb_show:1",
            "anidb:1:Q",
            "tvdb_show:1:x4",
        ] {
            assert_eq!(Descriptor::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn range_maps() {
        let table: [(&str, &str, u32, Option<u32>); 14] = [
            ("1-12", "13-24", 5, Some(17)),
            ("25-", "1-", 40, Some(16)),
            ("1-13", "25-37", 14, None),
            ("1-24", "1-15,17-22,24-26", 16, Some(17)),
            ("1-24", "1-15,17-22,24-26", 24, Some(26)),
            ("1-3", "1-12|-4", 2, Some(8)),
            ("1-3", "1-12|-4", 3, Some(12)),
            ("22-26", "1|5", 22, None),
            ("22-26", "1|5", 26, Some(1)),
            ("1-4", "1-2|2", 1, None),
            ("1-4", "1-2|2", 2, Some(1)),
            ("1-4", "1-2|2", 4, Some(2)),
            ("1", "1", 1, Some(1)),
            ("1", "1", 2, None),
        ];
        for (source, target, episode, expected) in table {
            let range = RangeMap::parse(source, target).unwrap();
            assert_eq!(
                range.map(episode),
                expected,
                "{source} -> {target} @ {episode}"
            );
        }
    }

    #[test]
    fn bad_ranges_are_rejected() {
        for (source, target) in [
            ("5-3", "1"),
            ("1", "x"),
            ("1", "1-,3"),
            ("1", "1|0"),
            ("1", "1|x"),
        ] {
            assert_eq!(
                RangeMap::parse(source, target),
                None,
                "{source} -> {target}"
            );
        }
    }

    #[test]
    fn fixture_loads() {
        let index =
            AniBridge::from_slice(include_bytes!("testdata/anibridge-fixture.json")).unwrap();
        assert_eq!(index.len(), 22);
        assert_eq!(index.generated_on(), Some("2026-10-23T06:49:48Z"));
        assert_eq!(
            index
                .edges(&Descriptor::TvdbShow {
                    id: 355774,
                    season: 4
                })
                .len(),
            10
        );
        assert!(AniBridge::from_slice(br#"{"$meta":{"schema_version":"4.0.0"}}"#).is_err());
        assert!(AniBridge::from_slice(br#"{"mal:1":{}}"#).is_err());
    }
}
