//! The per-episode sync markers (AL / MAL / Kitsu / Trakt chips): what each
//! service's list says about one episode. Pure, tested; `tracker` gathers
//! the reads and the queue, the UI draws the result.

use std::collections::HashMap;

use super::resolve::Resolution;
use super::resolve::ShowIds;
use super::rules::{self, ListStatus};
use super::tracker::target_of;
use super::trakt::ShowState;
use super::{EntryInfo, ServiceId};

/// The chips' left-to-right order.
pub const ORDER: [ServiceId; 4] = [
    ServiceId::AniList,
    ServiceId::Mal,
    ServiceId::Kitsu,
    ServiceId::Trakt,
];

/// The chip's text.
pub fn label(service: ServiceId) -> &'static str {
    match service {
        ServiceId::AniList => "AL",
        ServiceId::Mal => "MAL",
        ServiceId::Kitsu => "Kitsu",
        ServiceId::Trakt => "Trakt",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipState {
    /// The service's progress covers the episode.
    OnList,
    /// Matched, not on the list yet.
    NotYet,
    /// An update for it waits in `tracking_queue`.
    Queued,
    /// No entry to update (or nothing to match it to).
    Unmatched,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub service: ServiceId,
    pub state: ChipState,
    pub tooltip: String,
}

/// What reading a mapping service's entries gave.
#[derive(Clone, Copy)]
pub enum Read<'a> {
    Entries(&'a HashMap<u32, EntryInfo>),
    Failed(&'a str),
}

/// What reading a show's progress on Trakt gave.
#[derive(Clone, Copy)]
pub enum TraktRead<'a> {
    /// The show has no tvdb/tmdb/imdb id.
    NoIds,
    /// Trakt doesn't know the show.
    Unknown,
    Failed(&'a str),
    Show(&'a ShowState),
}

/// The chip of MAL, AniList or Kitsu for one resolved episode.
pub fn mapping_chip(service: ServiceId, resolution: &Resolution, queued: bool, read: Read) -> Chip {
    let name = service.short();
    let unmatched = |why: &str| Chip {
        service,
        state: ChipState::Unmatched,
        tooltip: format!("{name}: {why}"),
    };
    let found = match resolution {
        Resolution::Skipped { silent: true, .. } => return unmatched("specials aren't synced"),
        Resolution::Skipped { .. } => return unmatched("no match for this episode"),
        Resolution::Matched(found) => found,
    };
    let Some(target) = target_of(service, found) else {
        return unmatched("no entry for this episode");
    };
    let info = match read {
        Read::Entries(entries) => match entries.get(&target.id) {
            Some(info) => Some(info),
            None => return unmatched(&format!("not on {name}")),
        },
        Read::Failed(_) => None,
    };
    if target.extrapolated
        && let Some(info) = info
        && let Some(why) = rules::extrapolation_skip(info.total, info.airing, target.episode)
    {
        return unmatched(why);
    }
    let title = info
        .map(|i| i.title.as_str())
        .filter(|t| !t.is_empty())
        .map_or_else(|| format!("entry {}", target.id), str::to_string);
    let base = format!("{name}: {title} · ep {}", target.episode);
    let chip = |state, note: &str| Chip {
        service,
        state,
        tooltip: format!("{base} — {note}"),
    };
    if queued {
        return chip(ChipState::Queued, "update queued");
    }
    if let Read::Failed(why) = read {
        return chip(ChipState::NotYet, &format!("status unavailable ({why})"));
    }
    let entry = info.and_then(|i| i.entry);
    let on_list =
        entry.is_some_and(|e| e.progress >= target.episode || e.status == ListStatus::Completed);
    if on_list {
        chip(ChipState::OnList, "on your list")
    } else {
        let progress = entry.map_or(0, |e| e.progress);
        chip(
            ChipState::NotYet,
            &format!("not on your list yet (progress {progress})"),
        )
    }
}

/// Trakt's chip for an episode. `season` and `number` are Emby's (shown in
/// the tooltip); whether Trakt has it comes from the episode's own ids,
/// because Trakt numbers episodes differently. Without a Trakt episode for
/// those ids the chip is dimmed with the reason, never ✓ or the warning.
pub fn trakt_chip(
    show: &str,
    season: u32,
    number: u32,
    queued: bool,
    read: TraktRead,
    ids: &ShowIds,
    count: u32,
) -> Chip {
    let service = ServiceId::Trakt;
    let show = if show.is_empty() { "show" } else { show };
    let base = format!("Trakt: {show} · S{season}E{number}");
    let chip = |state, note: &str| Chip {
        service,
        state,
        tooltip: format!("{base} — {note}"),
    };
    if queued {
        return chip(ChipState::Queued, "update queued");
    }
    match read {
        TraktRead::NoIds => chip(
            ChipState::Unmatched,
            "the show has no TVDB, TMDB or IMDb id",
        ),
        TraktRead::Unknown => chip(ChipState::Unmatched, "Trakt doesn't know this show"),
        TraktRead::Failed(why) => chip(ChipState::NotYet, &format!("status unavailable ({why})")),
        TraktRead::Show(state) => match state.episode_watched(ids, count) {
            Some(true) => chip(ChipState::OnList, "watched on Trakt"),
            Some(false) => chip(ChipState::NotYet, "not watched on Trakt yet"),
            None => chip(
                ChipState::NotYet,
                "status unknown: no Trakt episode matches this episode's IDs",
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracking::resolve::{Match, Route, Target};
    use crate::tracking::rules::Entry;

    fn matched(mal: Option<(u32, u32)>, anilist: Option<(u32, u32)>) -> Resolution {
        let target = |(id, episode)| Target {
            id,
            episode,
            extrapolated: false,
        };
        Resolution::Matched(Match {
            mal: mal.map(target),
            anilist: anilist.map(target),
            route: Route::SeriesIds,
            suspect_season_anidb: None,
        })
    }

    fn entries(id: u32, progress: Option<u32>, status: ListStatus) -> HashMap<u32, EntryInfo> {
        HashMap::from([(
            id,
            EntryInfo {
                title: "Dr. Stone".into(),
                entry: progress.map(|progress| Entry { progress, status }),
                total: Some(12),
                airing: false,
            },
        )])
    }

    #[test]
    fn progress_decides_on_list_or_not_yet() {
        let found = matched(Some((100, 5)), None);
        let list = entries(100, Some(5), ListStatus::Watching);
        let chip = mapping_chip(ServiceId::Mal, &found, false, Read::Entries(&list));
        assert_eq!(chip.state, ChipState::OnList);
        assert_eq!(chip.tooltip, "MAL: Dr. Stone · ep 5 — on your list");

        let list = entries(100, Some(4), ListStatus::Watching);
        let chip = mapping_chip(ServiceId::Mal, &found, false, Read::Entries(&list));
        assert_eq!(chip.state, ChipState::NotYet);
        assert!(chip.tooltip.ends_with("not on your list yet (progress 4)"));

        let none = entries(100, None, ListStatus::Watching);
        let chip = mapping_chip(ServiceId::Mal, &found, false, Read::Entries(&none));
        assert_eq!(chip.state, ChipState::NotYet);

        let done = entries(100, Some(2), ListStatus::Completed);
        let chip = mapping_chip(ServiceId::Mal, &found, false, Read::Entries(&done));
        assert_eq!(chip.state, ChipState::OnList);
    }

    #[test]
    fn kitsu_follows_the_mal_target_and_anilist_its_own() {
        let found = matched(Some((100, 5)), Some((200, 3)));
        let list = entries(200, Some(3), ListStatus::Watching);
        let chip = mapping_chip(ServiceId::AniList, &found, false, Read::Entries(&list));
        assert_eq!(chip.state, ChipState::OnList);
        assert!(chip.tooltip.starts_with("AniList: Dr. Stone · ep 3"));
        let list = entries(100, Some(5), ListStatus::Watching);
        let chip = mapping_chip(ServiceId::Kitsu, &found, false, Read::Entries(&list));
        assert_eq!(chip.state, ChipState::OnList);
    }

    #[test]
    fn queued_wins_and_unmatched_says_why() {
        let found = matched(Some((100, 5)), None);
        let list = entries(100, Some(5), ListStatus::Watching);
        let chip = mapping_chip(ServiceId::Mal, &found, true, Read::Entries(&list));
        assert_eq!(chip.state, ChipState::Queued);

        let chip = mapping_chip(ServiceId::AniList, &found, false, Read::Entries(&list));
        assert_eq!(chip.state, ChipState::Unmatched);
        assert_eq!(chip.tooltip, "AniList: no entry for this episode");

        let special = Resolution::Skipped {
            silent: true,
            suspect_season_anidb: None,
        };
        let chip = mapping_chip(ServiceId::Mal, &special, false, Read::Entries(&list));
        assert_eq!(chip.tooltip, "MAL: specials aren't synced");
        let lost = Resolution::Skipped {
            silent: false,
            suspect_season_anidb: None,
        };
        let chip = mapping_chip(ServiceId::Mal, &lost, false, Read::Entries(&list));
        assert_eq!(chip.state, ChipState::Unmatched);
    }

    #[test]
    fn an_entry_the_service_lacks_is_unmatched() {
        let found = matched(Some((100, 5)), None);
        let list = entries(7, Some(5), ListStatus::Watching);
        let chip = mapping_chip(ServiceId::Kitsu, &found, false, Read::Entries(&list));
        assert_eq!(chip.state, ChipState::Unmatched);
        assert_eq!(chip.tooltip, "Kitsu: not on Kitsu");
    }

    #[test]
    fn an_extrapolated_episode_needs_room_or_an_airing_entry() {
        let found = |episode| {
            Resolution::Matched(Match {
                mal: Some(Target {
                    id: 100,
                    episode,
                    extrapolated: true,
                }),
                anilist: None,
                route: Route::SeriesIds,
                suspect_season_anidb: None,
            })
        };
        let info = |total, airing| {
            HashMap::from([(
                100,
                EntryInfo {
                    title: "Anbu".into(),
                    entry: None,
                    total,
                    airing,
                },
            )])
        };
        let chip_of = |episode, list: &HashMap<u32, EntryInfo>| {
            mapping_chip(ServiceId::Mal, &found(episode), false, Read::Entries(list))
        };
        // Within the entry's total: matched.
        assert_eq!(chip_of(3, &info(Some(12), false)).state, ChipState::NotYet);
        // Past it: unmatched.
        let chip = chip_of(13, &info(Some(12), true));
        assert_eq!(chip.state, ChipState::Unmatched);
        assert_eq!(chip.tooltip, "MAL: past the entry's last episode");
        // Unknown total: only while airing.
        assert_eq!(chip_of(3, &info(None, true)).state, ChipState::NotYet);
        assert_eq!(chip_of(3, &info(None, false)).state, ChipState::Unmatched);
    }

    #[test]
    fn a_failed_read_is_dimmed_with_the_reason() {
        let found = matched(Some((100, 5)), None);
        let chip = mapping_chip(ServiceId::Mal, &found, false, Read::Failed("offline"));
        assert_eq!(chip.state, ChipState::NotYet);
        assert!(chip.tooltip.contains("status unavailable (offline)"));
    }

    fn trakt_show(progress: &str) -> ShowState {
        let seasons = r#"[{"number":1,"episodes":[
            {"season":1,"number":12,"ids":{"tvdb":912}},
            {"season":1,"number":13,"ids":{"tvdb":913}}]}]"#;
        ShowState {
            watched: std::sync::Arc::new(crate::tracking::trakt::watched_from_progress(
                &serde_json::from_str(progress).unwrap(),
            )),
            episodes: std::sync::Arc::new(crate::tracking::trakt::episode_map_from_seasons(
                &serde_json::from_str(seasons).unwrap(),
            )),
        }
    }

    fn episode(tvdb: Option<u32>) -> ShowIds {
        ShowIds {
            tvdb,
            ..Default::default()
        }
    }

    const WATCHED_S1E12: &str =
        r#"{"seasons":[{"number":1,"episodes":[{"number":12,"completed":true}]}]}"#;

    #[test]
    fn trakt_chip_translates_emby_s2e1_to_trakt_s1e12() {
        let show = trakt_show(WATCHED_S1E12);
        // Emby calls it S2E1; Trakt's S1E12 is watched.
        let chip = trakt_chip(
            "Oshi no Ko",
            2,
            1,
            false,
            TraktRead::Show(&show),
            &episode(Some(912)),
            1,
        );
        assert_eq!(chip.state, ChipState::OnList);
        assert_eq!(chip.tooltip, "Trakt: Oshi no Ko · S2E1 — watched on Trakt");
        // The next episode isn't, though Emby's S1E12 numbers would be.
        let chip = trakt_chip(
            "Oshi no Ko",
            2,
            2,
            false,
            TraktRead::Show(&show),
            &episode(Some(913)),
            1,
        );
        assert_eq!(chip.state, ChipState::NotYet);
        let chip = trakt_chip(
            "Oshi no Ko",
            2,
            1,
            true,
            TraktRead::Show(&show),
            &episode(Some(912)),
            1,
        );
        assert_eq!(chip.state, ChipState::Queued);
    }

    #[test]
    fn an_episode_without_a_trakt_match_is_dimmed_never_checked_or_warned() {
        let show = trakt_show(WATCHED_S1E12);
        for ep in [episode(None), episode(Some(4242))] {
            let chip = trakt_chip("Show", 1, 12, false, TraktRead::Show(&show), &ep, 1);
            assert_eq!(chip.state, ChipState::NotYet);
            assert!(chip.tooltip.contains("status unknown"));
        }
    }

    #[test]
    fn trakt_show_level_states() {
        let ep = episode(Some(912));
        assert_eq!(
            trakt_chip("Show", 1, 1, false, TraktRead::Unknown, &ep, 1).state,
            ChipState::Unmatched
        );
        assert_eq!(
            trakt_chip("Show", 1, 1, false, TraktRead::NoIds, &ep, 1).state,
            ChipState::Unmatched
        );
        assert_eq!(
            trakt_chip("Show", 1, 1, false, TraktRead::Failed("offline"), &ep, 1).state,
            ChipState::NotYet
        );
    }

    #[test]
    fn chip_text_and_order() {
        let text: Vec<&str> = ORDER.iter().map(|&id| label(id)).collect();
        assert_eq!(text, ["AL", "MAL", "Kitsu", "Trakt"]);
    }
}
