//! Which MAL / AniList entry and episode number an Emby episode (or movie)
//! belongs to, from the item's external ids and the AniBridge mappings.

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use anyhow::Result;

use crate::emby::models::BaseItem;

use super::mapping::{AniBridge, AniDbScope, Descriptor, Edge, RangeMap};

/// External ids from an Emby item's `ProviderIds` (keys matched case-insensitively).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShowIds {
    pub tvdb: Option<u32>,
    pub tmdb: Option<u32>,
    pub imdb: Option<String>,
    pub anidb: Option<u32>,
    pub mal: Option<u32>,
    pub anilist: Option<u32>,
}

/// One episode as Emby numbers it (TVDB-style).
#[derive(Debug, Clone, Copy)]
pub struct EpisodeRef {
    pub season: u32,
    /// `IndexNumberEnd` when the file covers several, else `IndexNumber`.
    pub episode: u32,
    /// The season item's own `AniDB` id.
    pub season_anidb: Option<u32>,
    /// Seasons numbered 1 and up.
    pub regular_seasons: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    SeasonAniDb,
    SeriesIds,
    TvdbLookup,
    TmdbLookup,
    ItemIds,
    MovieLookup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub id: u32,
    pub episode: u32,
    /// AniBridge lags behind a show that is still airing: the episode lies
    /// past the end of the entry's last range and its offset was carried on.
    /// The service checks it against the entry's own episode count / status.
    pub extrapolated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub mal: Option<Target>,
    pub anilist: Option<Target>,
    /// The route that produced the first hit (shown on the series page).
    pub route: Route,
    /// A season AniDB id that belongs to another show (logged, shown).
    pub suspect_season_anidb: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Matched(Match),
    /// `silent`: specials, which skip without a toast.
    Skipped {
        silent: bool,
        suspect_season_anidb: Option<u32>,
    },
}

fn provider_id<'a>(ids: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    ids.iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

impl ShowIds {
    pub fn from_provider_ids(ids: &HashMap<String, String>) -> ShowIds {
        let number = |name: &str| provider_id(ids, name).and_then(|v| v.parse::<u32>().ok());
        ShowIds {
            tvdb: number("Tvdb"),
            tmdb: number("Tmdb"),
            imdb: provider_id(ids, "Imdb").map(str::to_owned),
            anidb: number("AniDB"),
            mal: number("MyAnimeList"),
            anilist: number("AniList"),
        }
    }
}

#[derive(Default)]
struct Found {
    mal: Option<Target>,
    anilist: Option<Target>,
}

impl Found {
    fn any(&self) -> bool {
        self.mal.is_some() || self.anilist.is_some()
    }

    /// Takes `other`'s targets where this has none; a mapped target beats an
    /// extrapolated one.
    fn fill_from(&mut self, other: Found) {
        let pick = |own: Option<Target>, other: Option<Target>| match (own, other) {
            (Some(own), Some(other)) if own.extrapolated && !other.extrapolated => Some(other),
            (own, other) => own.or(other),
        };
        self.mal = pick(self.mal, other.mal);
        self.anilist = pick(self.anilist, other.anilist);
    }
}

#[derive(Clone, Copy)]
enum Service {
    Mal,
    AniList,
}

fn service_id(service: Service, target: &Descriptor) -> Option<u32> {
    match (service, target) {
        (Service::Mal, Descriptor::Mal(id)) | (Service::AniList, Descriptor::AniList(id)) => {
            Some(*id)
        }
        _ => None,
    }
}

/// The one entry of `service` that `episode` maps to among `edges`; `None`
/// when there's none or several disagree (never guess).
fn unique_target(
    edges: &[Edge],
    service: Service,
    episode: u32,
    context: &Descriptor,
) -> Option<Target> {
    let mut hits = edges.iter().filter_map(|edge| {
        let id = service_id(service, &edge.target)?;
        edge.ranges
            .iter()
            .find_map(|range| range.map(episode))
            .map(|episode| Target {
                id,
                episode,
                extrapolated: false,
            })
    });
    let first = hits.next()?;
    if hits.any(|other| other != first) {
        tracing::warn!("AniBridge: {context} episode {episode} maps to several entries");
        return None;
    }
    Some(first)
}

/// An episode past the end of AniBridge's data for a show that is still
/// airing (the mappings are rebuilt daily and lag behind new episodes):
/// carries the offset of the last entry's last range on. Only when that entry
/// is the last one covering the show-season (no other edge has a range
/// starting after it) and the range is a plain offset (no ratio). `only`
/// restricts it to one entry. The service validates the result against the
/// entry's own episode count / airing status.
/// Logs a warning once per key per session (the resolver runs per episode).
fn warn_once(key: String, message: &str) {
    static SEEN: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);
    let first = SEEN.lock().unwrap_or_else(|e| e.into_inner()).insert(key);
    if first {
        tracing::warn!("{message}");
    }
}

/// An entry's last range: its source end and the range.
fn last_range(edge: &Edge) -> Option<(u32, &RangeMap)> {
    edge.ranges
        .iter()
        .filter_map(|range| range.source.end.map(|end| (end, range)))
        .max_by_key(|(end, _)| *end)
}

fn extrapolate(
    edges: &[Edge],
    service: Service,
    episode: u32,
    only: Option<&Descriptor>,
) -> Option<Target> {
    let of_service = |edge: &&Edge| service_id(service, &edge.target).is_some();
    // Only for an episode no range of this service covers.
    if edges.iter().filter(of_service).any(|edge| {
        edge.ranges
            .iter()
            .any(|range| range.source.contains(episode))
    }) {
        return None;
    }
    let end = edges
        .iter()
        .filter(of_service)
        .filter_map(last_range)
        .map(|(end, _)| end)
        .filter(|end| *end < episode)
        .max()?;
    let mut targets = edges
        .iter()
        .filter(of_service)
        .filter(|edge| only.is_none_or(|only| edge.target == *only))
        .filter_map(|edge| {
            let (last_end, range) = last_range(edge)?;
            if last_end != end {
                return None;
            }
            // A plain offset: one target span as long as the source span.
            let [target] = &*range.target else {
                return None;
            };
            let plain = range.ratio == 0
                && target
                    .end
                    .is_none_or(|t| t - target.start == last_end - range.source.start);
            if !plain {
                return None;
            }
            let shift = i64::from(target.start) - i64::from(range.source.start);
            let mapped = u32::try_from(i64::from(episode) + shift)
                .ok()
                .filter(|e| *e >= 1)?;
            let id = service_id(service, &edge.target)?;
            Some((
                edge,
                Target {
                    id,
                    episode: mapped,
                    extrapolated: true,
                },
            ))
        });
    let (winner, target) = targets.next()?;
    if targets.any(|(_, other)| other != target) {
        return None;
    }
    // The last entry: nothing else starts after its last range.
    let later = edges.iter().any(|edge| {
        edge.target != winner.target && edge.ranges.iter().any(|range| range.source.start > end)
    });
    if later {
        return None;
    }
    Some(target)
}

/// Direct edges of a show-season key first; its AniDB edges only for a
/// service with no direct edge (AniDB↔MAL links are wrong for some shows).
fn lookup(index: &AniBridge, key: &Descriptor, episode: u32) -> Found {
    let edges = index.edges(key);
    let mut found = Found {
        mal: unique_target(edges, Service::Mal, episode, key),
        anilist: unique_target(edges, Service::AniList, episode, key),
    };
    for service in [Service::Mal, Service::AniList] {
        let slot = match service {
            Service::Mal => &mut found.mal,
            Service::AniList => &mut found.anilist,
        };
        // Hop only when no direct edge of this service covers the episode.
        if slot.is_some() {
            continue;
        }
        let mut hops = edges
            .iter()
            .filter(|edge| matches!(edge.target, Descriptor::AniDb { .. }))
            .filter_map(|edge| {
                let anidb_episode = edge.ranges.iter().find_map(|range| range.map(episode))?;
                unique_target(
                    index.edges(&edge.target),
                    service,
                    anidb_episode,
                    &edge.target,
                )
            });
        if let Some(first) = hops.next()
            && hops.all(|other| other == first)
        {
            *slot = Some(first);
        }
    }
    // Last: an episode AniBridge doesn't have yet.
    found.mal = found
        .mal
        .or_else(|| extrapolate(edges, Service::Mal, episode, None));
    found.anilist = found
        .anilist
        .or_else(|| extrapolate(edges, Service::AniList, episode, None));
    found
}

pub fn resolve_episode(index: &AniBridge, series: &ShowIds, ep: &EpisodeRef) -> Resolution {
    // Emby numbers seasons like TVDB; TMDB's seasons often differ (Oshi no
    // Ko is one long TMDB season, its specials are TVDB's S3E1-2), so TMDB
    // keys only stand in for a show AniBridge doesn't know by TVDB.
    let tvdb_known = series.tvdb.is_some_and(|id| {
        (0..=ep.season.max(ep.regular_seasons))
            .any(|season| index.contains(&Descriptor::TvdbShow { id, season }))
    });
    let show_keys: Vec<(Descriptor, Route)> = [
        series.tvdb.map(|id| {
            (
                Descriptor::TvdbShow {
                    id,
                    season: ep.season,
                },
                Route::TvdbLookup,
            )
        }),
        series.tmdb.filter(|_| !tvdb_known).map(|id| {
            (
                Descriptor::TmdbShow {
                    id,
                    season: ep.season,
                },
                Route::TmdbLookup,
            )
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    let fill = |found: &mut Found| {
        for (key, _) in &show_keys {
            found.fill_from(lookup(index, key, ep.episode));
        }
    };
    let mut suspect = None;

    // 1. The season item's own AniDB id, when it belongs to this show.
    if let Some(anidb) = ep.season_anidb {
        let scope = if ep.season == 0 {
            AniDbScope::Special
        } else {
            AniDbScope::Regular
        };
        let key = Descriptor::AniDb { id: anidb, scope };
        if index.contains(&key) {
            let edges = index.edges(&key);
            let show_key = show_keys
                .iter()
                .map(|(show_key, _)| show_key)
                .find(|show_key| edges.iter().any(|edge| edge.target == **show_key));
            match show_key {
                Some(show_key) => {
                    let mut found = lookup(index, show_key, ep.episode);
                    fill(&mut found);
                    if found.any() {
                        return Resolution::Matched(Match {
                            mal: found.mal,
                            anilist: found.anilist,
                            route: Route::SeasonAniDb,
                            suspect_season_anidb: None,
                        });
                    }
                }
                None => {
                    suspect = Some(anidb);
                    warn_once(
                        format!("anidb:{anidb}:{:?}:{}", series.tvdb, ep.season),
                        &format!(
                            "season AniDB {anidb} maps to another show (series tvdb {:?}, S{})",
                            series.tvdb, ep.season
                        ),
                    );
                }
            }
        }
    }

    // 2. The series' own MAL / AniList ids, for a show with one season.
    if ep.season >= 1 && ep.regular_seasons == 1 {
        let mut found = Found::default();
        for (desc, service) in [
            (series.mal.map(Descriptor::Mal), Service::Mal),
            (series.anilist.map(Descriptor::AniList), Service::AniList),
        ] {
            let Some(desc) = desc else { continue };
            let hit = if !index.contains(&desc) {
                // Not in the mappings: trust the item.
                service_id(service, &desc).map(|id| Target {
                    id,
                    episode: ep.episode,
                    extrapolated: false,
                })
            } else if let Some((show_key, _)) = show_keys.iter().find(|(show_key, _)| {
                index
                    .edges(&desc)
                    .iter()
                    .any(|edge| edge.target == *show_key)
            }) {
                let all = index.edges(show_key);
                let edges: Vec<Edge> = all
                    .iter()
                    .filter(|edge| edge.target == desc)
                    .cloned()
                    .collect();
                unique_target(&edges, service, ep.episode, show_key)
                    .or_else(|| extrapolate(all, service, ep.episode, Some(&desc)))
            } else {
                warn_once(
                    format!("series:{desc}:{:?}", series.tvdb),
                    &format!("{desc} on the series item maps to another show"),
                );
                None
            };
            match service {
                Service::Mal => found.mal = hit,
                Service::AniList => found.anilist = hit,
            }
        }
        if found.any() {
            fill(&mut found);
            return Resolution::Matched(Match {
                mal: found.mal,
                anilist: found.anilist,
                route: Route::SeriesIds,
                suspect_season_anidb: suspect,
            });
        }
    }

    // 3. The show-season keys.
    let mut found = Found::default();
    let mut route = None;
    for (key, key_route) in &show_keys {
        let hit = lookup(index, key, ep.episode);
        if route.is_none() && hit.any() {
            route = Some(*key_route);
        }
        found.fill_from(hit);
    }
    if let Some(route) = route {
        return Resolution::Matched(Match {
            mal: found.mal,
            anilist: found.anilist,
            route,
            suspect_season_anidb: suspect,
        });
    }

    // 4. Nothing; specials skip without a toast.
    Resolution::Skipped {
        silent: ep.season == 0,
        suspect_season_anidb: suspect,
    }
}

pub fn resolve_movie(index: &AniBridge, movie: &ShowIds) -> Resolution {
    if movie.mal.is_some() || movie.anilist.is_some() {
        let target = |id: Option<u32>| {
            id.map(|id| Target {
                id,
                episode: 1,
                extrapolated: false,
            })
        };
        return Resolution::Matched(Match {
            mal: target(movie.mal),
            anilist: target(movie.anilist),
            route: Route::ItemIds,
            suspect_season_anidb: None,
        });
    }
    let keys = [
        movie.tmdb.map(Descriptor::TmdbMovie),
        movie
            .imdb
            .as_deref()
            .map(|id| Descriptor::ImdbMovie(id.into())),
        movie.tvdb.map(Descriptor::TvdbMovie),
    ];
    let mut found = Found::default();
    for key in keys.iter().flatten() {
        let edges = index.edges(key);
        found.fill_from(Found {
            mal: unique_target(edges, Service::Mal, 1, key),
            anilist: unique_target(edges, Service::AniList, 1, key),
        });
    }
    if found.any() {
        return Resolution::Matched(Match {
            mal: found.mal,
            anilist: found.anilist,
            route: Route::MovieLookup,
            suspect_season_anidb: None,
        });
    }
    Resolution::Skipped {
        silent: false,
        suspect_season_anidb: None,
    }
}

#[derive(Debug, Clone)]
pub struct EpisodeResolution {
    pub episode_id: String,
    pub season: u32,
    pub episode: u32,
    pub resolution: Resolution,
}

/// Each season's own AniDB id, and how many regular seasons there are.
fn season_facts(seasons: &[BaseItem]) -> (u32, HashMap<&str, Option<u32>>) {
    let regular = seasons
        .iter()
        .filter(|season| season.index_number >= Some(1))
        .count();
    let anidb = seasons
        .iter()
        .map(|season| {
            (
                season.id.as_str(),
                ShowIds::from_provider_ids(&season.provider_ids).anidb,
            )
        })
        .collect();
    (u32::try_from(regular).unwrap_or(u32::MAX), anidb)
}

/// The episode as the resolver wants it; `None` without season/episode numbers.
fn episode_ref(
    item: &BaseItem,
    regular_seasons: u32,
    season_anidb: &HashMap<&str, Option<u32>>,
) -> Option<EpisodeRef> {
    let season = item
        .parent_index_number
        .and_then(|n| u32::try_from(n).ok())?;
    let number = item.index_number.and_then(|n| u32::try_from(n).ok())?;
    let episode = item
        .index_number_end
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(number);
    let season_anidb = item
        .season_id
        .as_deref()
        .and_then(|id| season_anidb.get(id).copied().flatten());
    Some(EpisodeRef {
        season,
        episode,
        season_anidb,
        regular_seasons,
    })
}

/// Resolves every numbered episode of a series (the series page's tracking line).
pub async fn resolve_series(
    client: &crate::emby::EmbyClient,
    user_id: &str,
    series_id: &str,
    index: &AniBridge,
) -> Result<Vec<EpisodeResolution>> {
    let (series, seasons, episodes) = tokio::try_join!(
        client.item(user_id, series_id),
        client.seasons(series_id, user_id),
        client.series_episodes(series_id, user_id),
    )?;
    let ids = ShowIds::from_provider_ids(&series.provider_ids);
    let (regular_seasons, season_anidb) = season_facts(&seasons);
    Ok(episodes
        .iter()
        .filter_map(|item| {
            let ep = episode_ref(item, regular_seasons, &season_anidb)?;
            Some(EpisodeResolution {
                episode_id: item.id.clone(),
                season: ep.season,
                episode: ep.episode,
                resolution: resolve_episode(index, &ids, &ep),
            })
        })
        .collect())
}

/// One episode or movie, for a watched/unwatched update. `None` for other
/// item types or an episode without numbers.
pub async fn resolve_item(
    client: &crate::emby::EmbyClient,
    user_id: &str,
    item: &BaseItem,
    index: &AniBridge,
) -> Result<Option<EpisodeResolution>> {
    match item.item_type.as_str() {
        "Movie" => Ok(Some(EpisodeResolution {
            episode_id: item.id.clone(),
            season: 0,
            episode: 1,
            resolution: resolve_movie(index, &ShowIds::from_provider_ids(&item.provider_ids)),
        })),
        "Episode" => {
            let Some(series_id) = item.series_id.as_deref() else {
                return Ok(None);
            };
            let (series, seasons) = tokio::try_join!(
                client.item(user_id, series_id),
                client.seasons(series_id, user_id),
            )?;
            let (regular_seasons, season_anidb) = season_facts(&seasons);
            let Some(ep) = episode_ref(item, regular_seasons, &season_anidb) else {
                return Ok(None);
            };
            Ok(Some(EpisodeResolution {
                episode_id: item.id.clone(),
                season: ep.season,
                episode: ep.episode,
                resolution: resolve_episode(
                    index,
                    &ShowIds::from_provider_ids(&series.provider_ids),
                    &ep,
                ),
            }))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> AniBridge {
        AniBridge::from_slice(include_bytes!("testdata/anibridge-fixture.json")).unwrap()
    }

    fn t(id: u32, episode: u32) -> Option<Target> {
        Some(Target {
            id,
            episode,
            extrapolated: false,
        })
    }

    fn x(id: u32, episode: u32) -> Option<Target> {
        t(id, episode).map(|t| Target {
            extrapolated: true,
            ..t
        })
    }

    fn ep(
        season: u32,
        episode: u32,
        season_anidb: Option<u32>,
        regular_seasons: u32,
    ) -> EpisodeRef {
        EpisodeRef {
            season,
            episode,
            season_anidb,
            regular_seasons,
        }
    }

    fn matched(r: Resolution) -> Match {
        match r {
            Resolution::Matched(m) => m,
            other => panic!("expected a match, got {other:?}"),
        }
    }

    fn show(tvdb: Option<u32>, tmdb: Option<u32>) -> ShowIds {
        ShowIds {
            tvdb,
            tmdb,
            ..Default::default()
        }
    }

    #[test]
    fn airing_show_past_the_mapped_range_is_extrapolated() {
        let index = index();
        // Toaru Anbu no ITEM: AniBridge stops at E2, E3 has aired.
        let series = ShowIds {
            mal: Some(61014),
            anilist: Some(186742),
            ..show(Some(460309), None)
        };
        let m = matched(resolve_episode(&index, &series, &ep(1, 2, None, 1)));
        assert_eq!((m.mal, m.anilist), (t(61014, 2), t(186742, 2)));
        let m = matched(resolve_episode(&index, &series, &ep(1, 3, None, 1)));
        assert_eq!((m.mal, m.anilist), (x(61014, 3), x(186742, 3)));
        assert_eq!(m.route, Route::SeriesIds);
        // Through the show-season key alone.
        let series = show(Some(460309), None);
        let m = matched(resolve_episode(&index, &series, &ep(1, 4, None, 2)));
        assert_eq!((m.mal, m.anilist), (x(61014, 4), x(186742, 4)));
        assert_eq!(m.route, Route::TvdbLookup);
    }

    #[test]
    fn only_the_last_entry_of_a_split_season_is_extrapolated() {
        let index = index();
        let series = show(Some(900001), None);
        // The later entry starts at E25: E20 belongs to neither.
        assert!(matches!(
            resolve_episode(&index, &series, &ep(1, 20, None, 2)),
            Resolution::Skipped { .. }
        ));
        let m = matched(resolve_episode(&index, &series, &ep(1, 25, None, 2)));
        assert_eq!((m.mal, m.anilist), (t(9002, 1), t(8002, 1)));
        // Past the last entry: it carries on, never the earlier one.
        let m = matched(resolve_episode(&index, &series, &ep(1, 40, None, 2)));
        assert_eq!((m.mal, m.anilist), (x(9002, 16), x(8002, 16)));
    }

    #[test]
    fn a_ratio_is_never_extrapolated() {
        let index = index();
        let series = show(Some(900002), None);
        let m = matched(resolve_episode(&index, &series, &ep(1, 12, None, 1)));
        assert_eq!(m.mal, t(9100, 24));
        assert!(matches!(
            resolve_episode(&index, &series, &ep(1, 13, None, 1)),
            Resolution::Skipped { .. }
        ));
    }

    #[test]
    fn dr_stone_season_four_has_three_entries() {
        let index = index();
        let series = show(Some(355774), Some(86031));
        let cases = [
            (1, t(57592, 1), t(172019, 1)),
            (13, t(61322, 1), t(189117, 1)),
            (25, t(62568, 1), t(199221, 1)),
            (37, t(62568, 13), t(199221, 13)),
        ];
        for (episode, mal, anilist) in cases {
            let m = matched(resolve_episode(
                &index,
                &series,
                &ep(4, episode, Some(18336), 4),
            ));
            assert_eq!((m.mal, m.anilist), (mal, anilist), "E{episode}");
            assert_eq!(m.route, Route::SeasonAniDb);
            assert_eq!(m.suspect_season_anidb, None);
        }
    }

    #[test]
    fn bungou_direct_edges_win_over_anidb() {
        let index = index();
        let series = show(Some(305075), Some(65931));
        let m = matched(resolve_episode(&index, &series, &ep(1, 1, Some(11523), 5)));
        assert_eq!((m.mal, m.anilist), (t(31478, 1), t(21311, 1)));
        assert_eq!(m.route, Route::SeasonAniDb);
        let m = matched(resolve_episode(&index, &series, &ep(1, 13, Some(11523), 5)));
        assert_eq!((m.mal, m.anilist), (t(32867, 1), t(21679, 1)));
        assert_eq!(m.route, Route::SeasonAniDb);
    }

    #[test]
    fn oshi_no_ko_wrong_season_anidb_is_flagged() {
        let index = index();
        let series = ShowIds {
            mal: Some(52034),
            ..show(Some(421069), Some(203737))
        };
        for (episode, id) in [(1, 1), (11, 11)] {
            let m = matched(resolve_episode(
                &index,
                &series,
                &ep(1, episode, Some(14087), 3),
            ));
            assert_eq!((m.mal, m.anilist), (t(52034, id), t(150672, id)));
            assert_eq!(m.route, Route::TvdbLookup);
            assert_eq!(m.suspect_season_anidb, Some(14087));
        }
    }

    #[test]
    fn single_season_series_ids() {
        let index = index();
        // Unlisted show: trust the item's ids.
        let series = ShowIds {
            mal: Some(1234),
            anilist: Some(5678),
            ..show(Some(999001), None)
        };
        let m = matched(resolve_episode(&index, &series, &ep(1, 5, None, 1)));
        assert_eq!(
            (m.mal, m.anilist, m.route),
            (t(1234, 5), t(5678, 5), Route::SeriesIds)
        );
        // Listed and consistent with the TVDB key.
        let series = ShowIds {
            mal: Some(1),
            ..show(Some(76885), None)
        };
        let m = matched(resolve_episode(&index, &series, &ep(1, 26, None, 1)));
        assert_eq!(
            (m.mal, m.anilist, m.route),
            (t(1, 26), t(1, 26), Route::SeriesIds)
        );
        // Listed, but it belongs to another show.
        let series = ShowIds {
            mal: Some(1),
            ..show(Some(999002), None)
        };
        assert_eq!(
            resolve_episode(&index, &series, &ep(1, 3, None, 1)),
            Resolution::Skipped {
                silent: false,
                suspect_season_anidb: None
            }
        );
        // Two regular seasons: the series ids don't say which season.
        let series = ShowIds {
            mal: Some(1234),
            ..show(Some(999001), None)
        };
        assert!(matches!(
            resolve_episode(&index, &series, &ep(1, 5, None, 2)),
            Resolution::Skipped { silent: false, .. }
        ));
    }

    #[test]
    fn specials_hit_and_miss() {
        let index = index();
        let series = show(Some(76885), Some(30991));
        let m = matched(resolve_episode(&index, &series, &ep(0, 3, None, 1)));
        assert_eq!(
            (m.mal, m.anilist, m.route),
            (t(5, 1), t(5, 1), Route::TvdbLookup)
        );
        let m = matched(resolve_episode(&index, &series, &ep(0, 1, None, 1)));
        assert_eq!((m.mal, m.anilist), (t(4037, 1), t(4037, 1)));
        assert_eq!(
            resolve_episode(&index, &series, &ep(0, 2, None, 1)),
            Resolution::Skipped {
                silent: true,
                suspect_season_anidb: None
            }
        );
    }

    #[test]
    fn tmdb_keys_only_for_shows_unknown_by_tvdb() {
        let index = index();
        // TMDB's Oshi no Ko S0E1-2 are TVDB's S3E1-2, not Emby's specials.
        assert_eq!(
            resolve_episode(
                &index,
                &show(Some(421069), Some(203737)),
                &ep(0, 1, None, 3)
            ),
            Resolution::Skipped {
                silent: true,
                suspect_season_anidb: None
            }
        );
        let m = matched(resolve_episode(
            &index,
            &show(None, Some(203737)),
            &ep(0, 1, None, 3),
        ));
        assert_eq!(
            (m.mal, m.anilist, m.route),
            (None, t(182587, 1), Route::TmdbLookup)
        );
    }

    #[test]
    fn unlisted_show_is_skipped() {
        assert_eq!(
            resolve_episode(&index(), &show(Some(999001), None), &ep(1, 1, None, 3)),
            Resolution::Skipped {
                silent: false,
                suspect_season_anidb: None
            }
        );
    }

    #[test]
    fn movies() {
        let index = index();
        let m = matched(resolve_movie(&index, &show(None, Some(11299))));
        assert_eq!(
            (m.mal, m.anilist, m.route),
            (t(5, 1), t(5, 1), Route::MovieLookup)
        );
        let ids = ShowIds {
            mal: Some(99),
            ..show(None, Some(11299))
        };
        let m = matched(resolve_movie(&index, &ids));
        assert_eq!(
            (m.mal, m.anilist, m.route),
            (t(99, 1), None, Route::ItemIds)
        );
    }

    #[test]
    fn provider_ids_ignore_key_casing() {
        let ids: HashMap<String, String> = [
            ("anidb", "14087"),
            ("MyAnimeList", "52034"),
            ("anilist", "150672"),
            ("Tvdb", "421069"),
            ("Tmdb", " 203737 "),
            ("Imdb", "tt123"),
            ("AniDB2", "1"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        assert_eq!(
            ShowIds::from_provider_ids(&ids),
            ShowIds {
                tvdb: Some(421069),
                tmdb: Some(203737),
                imdb: Some("tt123".into()),
                anidb: Some(14087),
                mal: Some(52034),
                anilist: Some(150672),
            }
        );
        let junk: HashMap<String, String> = [("Tvdb".to_owned(), "abc".to_owned())].into();
        assert_eq!(ShowIds::from_provider_ids(&junk).tvdb, None);
    }
}
