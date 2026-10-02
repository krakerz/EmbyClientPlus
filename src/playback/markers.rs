//! Chapters and Emby's intro/credits markers, in seconds.

use crate::emby::models::{BaseItem, ChapterInfo};

use super::TICKS_PER_SECOND;

/// How long before the end the Up Next card appears when Emby has no
/// credits marker.
const UP_NEXT_FALLBACK_SECONDS: f64 = 20.0;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Markers {
    /// Regular chapter starts, for seek-bar marks.
    pub chapters: Vec<f64>,
    pub intro: Option<(f64, f64)>,
    pub credits: Option<f64>,
}

impl Markers {
    pub fn from_chapters(chapters: &[ChapterInfo]) -> Self {
        let seconds = |c: &ChapterInfo| c.start_position_ticks as f64 / TICKS_PER_SECOND as f64;
        let marker = |kind: &str| {
            chapters
                .iter()
                .find(|c| c.marker_type.as_deref() == Some(kind))
                .map(seconds)
        };
        let intro = match (marker("IntroStart"), marker("IntroEnd")) {
            (Some(start), Some(end)) if end > start => Some((start, end)),
            _ => None,
        };
        Markers {
            chapters: chapters
                .iter()
                .filter(|c| matches!(c.marker_type.as_deref(), None | Some("Chapter")))
                .map(seconds)
                .collect(),
            intro,
            credits: marker("CreditsStart"),
        }
    }

    /// Whether "Skip Intro" applies at `position`.
    pub fn in_intro(&self, position: f64) -> bool {
        self.intro
            .is_some_and(|(start, end)| position >= start && position < end - 1.0)
    }

    pub fn in_credits(&self, position: f64) -> bool {
        self.credits.is_some_and(|start| position >= start)
    }

    /// When the Up Next card should show for a file of `duration`.
    pub fn up_next_at(&self, duration: f64) -> f64 {
        self.credits
            .unwrap_or(duration - UP_NEXT_FALLBACK_SECONDS)
            .max(0.0)
    }

    /// The chapter start after/before `position`, for chapter keys.
    pub fn next_chapter(&self, position: f64) -> Option<f64> {
        self.chapters.iter().copied().find(|&c| c > position + 1.0)
    }

    pub fn previous_chapter(&self, position: f64) -> Option<f64> {
        // A little slack so pressing it right after a chapter start goes to
        // the one before, like most players.
        self.chapters
            .iter()
            .copied()
            .rev()
            .find(|&c| c < position - 3.0)
    }
}

/// The episodes before and after `item_id` in a series' watch order.
pub fn neighbours(episodes: &[BaseItem], item_id: &str) -> (Option<BaseItem>, Option<BaseItem>) {
    let Some(position) = episodes.iter().position(|e| e.id == item_id) else {
        return (None, None);
    };
    let previous = position
        .checked_sub(1)
        .and_then(|i| episodes.get(i))
        .cloned();
    (previous, episodes.get(position + 1).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapter(seconds: i64, marker: &str) -> ChapterInfo {
        ChapterInfo {
            start_position_ticks: seconds * TICKS_PER_SECOND,
            name: None,
            marker_type: Some(marker.into()),
        }
    }

    #[test]
    fn markers_split_chapters_intro_and_credits() {
        let markers = Markers::from_chapters(&[
            chapter(0, "Chapter"),
            chapter(30, "IntroStart"),
            chapter(120, "IntroEnd"),
            chapter(300, "Chapter"),
            chapter(1300, "CreditsStart"),
        ]);
        assert_eq!(markers.chapters, [0.0, 300.0]);
        assert_eq!(markers.intro, Some((30.0, 120.0)));
        assert!(markers.in_intro(60.0));
        assert!(!markers.in_intro(119.5));
        assert!(!markers.in_intro(10.0));
        assert!(markers.in_credits(1301.0));
        assert_eq!(markers.up_next_at(1420.0), 1300.0);
    }

    #[test]
    fn up_next_falls_back_to_the_last_seconds() {
        let markers = Markers::from_chapters(&[chapter(0, "Chapter")]);
        assert_eq!(markers.intro, None);
        assert_eq!(markers.up_next_at(1420.0), 1400.0);
        assert_eq!(markers.up_next_at(5.0), 0.0);
    }

    #[test]
    fn chapter_navigation() {
        let markers = Markers::from_chapters(&[
            chapter(0, "Chapter"),
            chapter(300, "Chapter"),
            chapter(600, "Chapter"),
        ]);
        assert_eq!(markers.next_chapter(10.0), Some(300.0));
        assert_eq!(markers.next_chapter(650.0), None);
        assert_eq!(markers.previous_chapter(301.0), Some(0.0));
        assert_eq!(markers.previous_chapter(400.0), Some(300.0));
    }

    #[test]
    fn neighbours_around_an_episode() {
        let episodes: Vec<BaseItem> = ["a", "b", "c"]
            .into_iter()
            .map(|id| BaseItem {
                id: id.into(),
                ..Default::default()
            })
            .collect();
        let (prev, next) = neighbours(&episodes, "b");
        assert_eq!(prev.unwrap().id, "a");
        assert_eq!(next.unwrap().id, "c");
        let (prev, next) = neighbours(&episodes, "a");
        assert!(prev.is_none());
        assert_eq!(next.unwrap().id, "b");
        assert_eq!(neighbours(&episodes, "zzz").1.map(|e| e.id), None);
    }
}
