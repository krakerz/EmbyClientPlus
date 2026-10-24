//! The list rules: never lower progress, first episode → watching, last
//! episode → completed, completed left alone; unwatching only lowers.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStatus {
    Watching,
    Completed,
    PlanToWatch,
    OnHold,
    Dropped,
    /// AniList REPEATING and anything unknown: left alone.
    Other,
}

/// The entry as the service has it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub progress: u32,
    pub status: ListStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Write(Entry),
    Unchanged,
    Skip(String),
}

/// Episode `episode` of an entry with `total` episodes (None: unknown) was watched.
pub fn watched(current: Option<Entry>, episode: u32, total: Option<u32>) -> Plan {
    if episode == 0 || total.is_some_and(|total| total > 0 && episode > total) {
        return Plan::Skip(format!("episode {episode} is beyond the entry's {total:?}"));
    }
    let progress = current.map_or(0, |c| c.progress);
    match current.map(|c| c.status) {
        Some(ListStatus::Completed | ListStatus::Other) => return Plan::Unchanged,
        _ if progress >= episode => return Plan::Unchanged,
        _ => {}
    }
    let status = if total == Some(episode) {
        ListStatus::Completed
    } else {
        ListStatus::Watching
    };
    Plan::Write(Entry {
        progress: episode,
        status,
    })
}

/// Episode `episode` was marked unwatched ("Also sync unwatched").
pub fn unwatched(current: Option<Entry>, episode: u32) -> Plan {
    match current {
        Some(c) if episode > 0 && c.progress >= episode => Plan::Write(Entry {
            progress: episode - 1,
            status: if c.status == ListStatus::Completed {
                ListStatus::Watching
            } else {
                c.status
            },
        }),
        _ => Plan::Unchanged,
    }
}

/// Why an extrapolated episode (past the end of AniBridge's last range for
/// the entry) is still not sent, from the entry's own read: `total` (None or
/// 0: unknown) and whether it is airing now. `None`: allowed.
pub fn extrapolation_skip(total: Option<u32>, airing: bool, episode: u32) -> Option<&'static str> {
    match total.filter(|total| *total > 0) {
        Some(total) if episode > total => Some("past the entry's last episode"),
        Some(_) => None,
        None if airing => None,
        None => Some("episode count unknown and the entry isn't airing"),
    }
}

pub fn movie_watched(current: Option<Entry>) -> Plan {
    watched(current, 1, Some(1))
}

/// Back to plan-to-watch, but only from completed.
pub fn movie_unwatched(current: Option<Entry>) -> Plan {
    match current {
        Some(c) if c.status == ListStatus::Completed => Plan::Write(Entry {
            progress: 0,
            status: ListStatus::PlanToWatch,
        }),
        _ => Plan::Unchanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(progress: u32, status: ListStatus) -> Option<Entry> {
        Some(Entry { progress, status })
    }

    fn write(progress: u32, status: ListStatus) -> Plan {
        Plan::Write(Entry { progress, status })
    }

    #[test]
    fn first_and_last_episode() {
        assert_eq!(watched(None, 1, Some(12)), write(1, ListStatus::Watching));
        assert_eq!(
            watched(None, 12, Some(12)),
            write(12, ListStatus::Completed)
        );
    }

    #[test]
    fn never_lowers_progress() {
        let current = entry(5, ListStatus::Watching);
        assert_eq!(watched(current, 3, Some(12)), Plan::Unchanged);
        assert_eq!(
            watched(current, 6, Some(12)),
            write(6, ListStatus::Watching)
        );
    }

    #[test]
    fn completed_and_other_are_left_alone() {
        assert_eq!(
            watched(entry(12, ListStatus::Completed), 5, Some(12)),
            Plan::Unchanged
        );
        assert_eq!(
            watched(entry(3, ListStatus::Other), 5, Some(12)),
            Plan::Unchanged
        );
    }

    #[test]
    fn beyond_the_total_is_skipped() {
        assert!(matches!(watched(None, 13, Some(12)), Plan::Skip(_)));
        assert_eq!(watched(None, 30, None), write(30, ListStatus::Watching));
        assert_eq!(watched(None, 30, Some(0)), write(30, ListStatus::Watching));
    }

    #[test]
    fn plan_to_watch_and_on_hold_start_watching() {
        assert_eq!(
            watched(entry(0, ListStatus::PlanToWatch), 1, Some(12)),
            write(1, ListStatus::Watching)
        );
        assert_eq!(
            watched(entry(4, ListStatus::OnHold), 5, Some(12)),
            write(5, ListStatus::Watching)
        );
    }

    #[test]
    fn unwatching_only_lowers() {
        assert_eq!(
            unwatched(entry(8, ListStatus::Watching), 5),
            write(4, ListStatus::Watching)
        );
        assert_eq!(
            unwatched(entry(3, ListStatus::Watching), 5),
            Plan::Unchanged
        );
        assert_eq!(
            unwatched(entry(12, ListStatus::Completed), 12),
            write(11, ListStatus::Watching)
        );
        assert_eq!(unwatched(None, 3), Plan::Unchanged);
    }

    #[test]
    fn extrapolation_needs_room_or_an_airing_entry() {
        assert_eq!(extrapolation_skip(Some(12), false, 12), None);
        assert!(extrapolation_skip(Some(12), true, 13).is_some());
        assert_eq!(extrapolation_skip(None, true, 3), None);
        assert_eq!(extrapolation_skip(Some(0), true, 3), None);
        assert!(extrapolation_skip(None, false, 3).is_some());
        assert!(extrapolation_skip(Some(0), false, 3).is_some());
    }

    #[test]
    fn movies() {
        assert_eq!(movie_watched(None), write(1, ListStatus::Completed));
        assert_eq!(
            movie_unwatched(entry(1, ListStatus::Completed)),
            write(0, ListStatus::PlanToWatch)
        );
        assert_eq!(
            movie_unwatched(entry(0, ListStatus::Watching)),
            Plan::Unchanged
        );
    }
}
