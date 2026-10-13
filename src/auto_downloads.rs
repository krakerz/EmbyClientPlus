//! Automatic downloads: series whose next unwatched episodes are kept
//! downloaded (the original files, as with any download), with watched
//! ones optionally deleted. The jobs are this app's own, per Emby user, in
//! `auto-downloads.json` in the config folder; the UI runs them
//! (`ui::downloads::sync_auto`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::downloads::Download;
use crate::emby::models::BaseItem;

/// How many unwatched episodes a new job keeps.
pub const DEFAULT_KEEP: u32 = 3;
pub const MAX_KEEP: u32 = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub user_id: String,
    pub series_id: String,
    pub series_name: String,
    /// Unwatched episodes kept downloaded.
    pub keep: u32,
    /// Watched episodes' downloads are deleted.
    pub delete_watched: bool,
}

fn path() -> Result<PathBuf> {
    Ok(crate::config::config_dir()?.join("auto-downloads.json"))
}

fn load_all() -> Vec<Job> {
    path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_all(jobs: &[Job]) -> Result<()> {
    let path = path()?;
    std::fs::write(&path, serde_json::to_string_pretty(jobs)?)
        .with_context(|| format!("writing {}", path.display()))
}

/// `user_id`'s jobs, by series name.
pub fn for_user(user_id: &str) -> Vec<Job> {
    let mut jobs: Vec<Job> = load_all()
        .into_iter()
        .filter(|job| job.user_id == user_id)
        .collect();
    jobs.sort_by_key(|job| job.series_name.to_lowercase());
    jobs
}

pub fn get(user_id: &str, series_id: &str) -> Option<Job> {
    load_all()
        .into_iter()
        .find(|job| job.user_id == user_id && job.series_id == series_id)
}

/// Adds `job`, or replaces the one for the same user and series.
pub fn set(job: Job) -> Result<()> {
    let mut jobs = load_all();
    jobs.retain(|j| !(j.user_id == job.user_id && j.series_id == job.series_id));
    jobs.push(job);
    save_all(&jobs)
}

/// Stops auto-downloading a series (its downloads stay).
pub fn remove(user_id: &str, series_id: &str) -> Result<()> {
    let mut jobs = load_all();
    jobs.retain(|j| !(j.user_id == user_id && j.series_id == series_id));
    save_all(&jobs)
}

/// What a job wants done now.
#[derive(Debug, Default)]
pub struct Plan {
    /// Episodes to download, in order (some may be downloaded already).
    pub download: Vec<BaseItem>,
    /// Downloads of watched episodes to delete.
    pub delete: Vec<Download>,
}

/// From the series' `episodes` (in order, with watch state) and its
/// `downloaded` episodes: the next `keep` unwatched episodes after the
/// last watched one (specials left out, as in Next Up), and, if the job
/// says so, the watched downloads except `playing`.
pub fn plan(
    job: &Job,
    episodes: &[BaseItem],
    downloaded: &[Download],
    playing: Option<&str>,
) -> Plan {
    let regular: Vec<&BaseItem> = episodes
        .iter()
        .filter(|e| e.parent_index_number != Some(0))
        .collect();
    let after_last_watched = regular
        .iter()
        .rposition(|e| e.played())
        .map_or(0, |index| index + 1);
    let download = regular[after_last_watched..]
        .iter()
        .filter(|e| !e.played())
        .take(job.keep as usize)
        .map(|e| (*e).clone())
        .collect();
    let delete = if job.delete_watched {
        downloaded
            .iter()
            .filter(|d| Some(d.item.id.as_str()) != playing)
            .filter(|d| episodes.iter().any(|e| e.id == d.item.id && e.played()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    Plan { download, delete }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emby::models::UserItemData;

    fn episode(id: &str, season: i32, played: bool) -> BaseItem {
        BaseItem {
            id: id.into(),
            item_type: "Episode".into(),
            parent_index_number: Some(season),
            user_data: Some(UserItemData {
                played,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn downloaded(item: BaseItem) -> Download {
        Download {
            item,
            file: PathBuf::new(),
            image: None,
        }
    }

    fn job(keep: u32, delete_watched: bool) -> Job {
        Job {
            user_id: "u".into(),
            series_id: "s".into(),
            series_name: "Show".into(),
            keep,
            delete_watched,
        }
    }

    fn ids(items: &[BaseItem]) -> Vec<&str> {
        items.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn keeps_the_next_unwatched_after_the_last_watched() {
        let episodes = [
            episode("sp", 0, false),
            episode("1", 1, true),
            episode("2", 1, false),
            episode("3", 1, true),
            episode("4", 1, false),
            episode("5", 2, false),
            episode("6", 2, false),
        ];
        let plan = plan(&job(2, false), &episodes, &[], None);
        // Episode 2 was skipped over; specials don't count.
        assert_eq!(ids(&plan.download), ["4", "5"]);
        assert!(plan.delete.is_empty());
        let nothing_watched = [episode("1", 1, false), episode("2", 1, false)];
        assert_eq!(
            ids(&super::plan(&job(5, false), &nothing_watched, &[], None).download),
            ["1", "2"]
        );
    }

    #[test]
    fn deletes_watched_downloads_but_not_the_one_playing() {
        let episodes = [
            episode("1", 1, true),
            episode("2", 1, true),
            episode("3", 1, false),
        ];
        let have = [
            downloaded(episodes[0].clone()),
            downloaded(episodes[1].clone()),
            downloaded(episodes[2].clone()),
        ];
        let plan_on = plan(&job(1, true), &episodes, &have, Some("2"));
        let deleted: Vec<&str> = plan_on.delete.iter().map(|d| d.item.id.as_str()).collect();
        assert_eq!(deleted, ["1"]);
        assert!(
            plan(&job(1, false), &episodes, &have, None)
                .delete
                .is_empty()
        );
    }
}
