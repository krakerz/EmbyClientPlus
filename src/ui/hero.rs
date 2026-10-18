//! The header shared by series and details pages: fan-art banner, then
//! poster beside title, meta line, buttons and overview. `extra` holds
//! whatever the page shows below (episodes, seasons).

use std::cell::RefCell;

use adw::prelude::*;
use gtk::glib;

use super::card::Shape;
use super::rows::{Click, More, row};
use super::{Ui, banner, fixed_picture, format_runtime, images};
use crate::emby::browse::ItemQuery;
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

const POSTER_WIDTH: i32 = 200;
const POSTER_HEIGHT: i32 = 300;
/// Episodes show their own 16:9 still instead of the series poster.
const STILL_WIDTH: i32 = 384;
const STILL_HEIGHT: i32 = 216;

pub struct Hero {
    pub root: gtk::Box,
    backdrop: gtk::Picture,
    poster: gtk::Picture,
    poster_placeholder: gtk::Image,
    poster_frame: gtk::Overlay,
    still: gtk::Picture,
    still_frame: gtk::Overlay,
    /// Link above the title (the series, on an episode page).
    kicker: gtk::Button,
    kicker_handler: RefCell<Option<glib::SignalHandlerId>>,
    title: gtk::Label,
    meta: gtk::Label,
    pub buttons: gtk::Box,
    tagline: gtk::Label,
    overview: gtk::Label,
    /// Original title, studios, tags...: a row each, when the server has it.
    facts: gtk::Grid,
    /// The overview and the facts: on episode pages it keeps room for a few
    /// lines, so stepping between episodes doesn't move what's below.
    details: gtk::Box,
    pub extra: gtk::Box,
}

impl Hero {
    pub fn new() -> Self {
        let (backdrop, banner) = banner();
        let poster = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .build();
        let poster_frame = gtk::Overlay::builder()
            .child(&fixed_picture(&poster, POSTER_WIDTH, POSTER_HEIGHT))
            .overflow(gtk::Overflow::Hidden)
            .valign(gtk::Align::Start)
            .css_classes(["card"])
            .build();
        let poster_placeholder = super::placeholder_for(&poster, crate::ui::icons::IMAGE);
        poster_frame.add_overlay(&poster_placeholder);
        let still = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .build();
        let still_frame = gtk::Overlay::builder()
            .child(&fixed_picture(&still, STILL_WIDTH, STILL_HEIGHT))
            .overflow(gtk::Overflow::Hidden)
            .valign(gtk::Align::Start)
            .css_classes(["card"])
            .visible(false)
            .build();
        still_frame.add_overlay(&super::placeholder_for(&still, crate::ui::icons::EPISODE));
        let kicker = gtk::Button::builder()
            .halign(gtk::Align::Start)
            .css_classes(["flat", "heading", "hero-kicker"])
            .visible(false)
            .build();
        let title = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["title-1"])
            .build();
        let meta = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        let buttons = gtk::Box::builder().spacing(12).margin_top(6).build();
        // Word-or-character wrapping: a long unbroken string (a URL) can't
        // widen the page either.
        let tagline = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label"])
            .visible(false)
            .build();
        let overview = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .valign(gtk::Align::Start)
            .css_classes(["body"])
            .build();
        let facts = gtk::Grid::builder()
            .column_spacing(18)
            .row_spacing(4)
            .valign(gtk::Align::Start)
            .visible(false)
            .build();
        // The table beside the overview on a wide window, under it otherwise.
        let details = gtk::Box::new(gtk::Orientation::Vertical, 0);
        details.set_layout_manager(Some(super::side_by_side::SideBySide::new()));
        details.append(&overview);
        details.append(&facts);

        let info = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .hexpand(true)
            .build();
        info.append(&kicker);
        info.append(&title);
        info.append(&meta);
        info.append(&buttons);
        info.append(&tagline);
        info.append(&details);
        // Poster (or still), then the text column with the rest of the width.
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top.set_layout_manager(Some(super::side_by_side::FillLast::new()));
        top.append(&poster_frame);
        top.append(&still_frame);
        top.append(&info);

        let extra = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .build();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .build();
        body.append(&top);
        body.append(&extra);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&banner);
        root.append(
            &adw::Clamp::builder()
                .maximum_size(1400)
                .margin_top(24)
                .margin_bottom(24)
                .margin_start(18)
                .margin_end(18)
                .child(&body)
                .build(),
        );
        Hero {
            root,
            backdrop,
            poster,
            poster_placeholder,
            poster_frame,
            still,
            still_frame,
            kicker,
            kicker_handler: RefCell::new(None),
            title,
            meta,
            buttons,
            tagline,
            overview,
            facts,
            details,
            extra,
        }
    }

    /// Fills in `item`; `kicker` is a page to link above the title.
    pub fn show(&self, ui: &Ui, item: &BaseItem, kicker: Option<BaseItem>) {
        self.title.set_label(&item.episode_label());
        self.poster_placeholder
            .set_icon_name(Some(crate::ui::icons::placeholder(&item.item_type)));
        self.meta.set_label(&meta_line(item));
        let overview = super::plain_spaces(item.overview.as_deref().unwrap_or_default());
        self.overview.set_label(&overview);
        self.overview.set_visible(!overview.is_empty());
        let tagline =
            super::plain_spaces(item.taglines.first().map(|t| t.trim()).unwrap_or_default());
        self.tagline
            .set_markup(&format!("<i>{}</i>", glib::markup_escape_text(&tagline)));
        self.tagline.set_visible(!tagline.is_empty());
        self.show_facts(item);
        let reserved = if item.item_type == "Episode" {
            let (_, line) = self.overview.create_pango_layout(Some("Ag")).pixel_size();
            line * EPISODE_OVERVIEW_LINES
        } else {
            -1
        };
        self.details.set_height_request(reserved);
        images::load(ui, &self.backdrop, item.backdrop(), 1920);
        let still = (item.item_type == "Episode")
            .then(|| item.image_tags.get("Primary").map(|_| item.landscape()))
            .flatten()
            .flatten();
        self.still_frame.set_visible(still.is_some());
        self.poster_frame.set_visible(still.is_none());
        match still {
            Some(still) => images::load(ui, &self.still, Some(still), STILL_WIDTH as u32 * 2),
            None => images::load(ui, &self.poster, item.poster(), POSTER_WIDTH as u32 * 2),
        }

        if let Some(id) = self.kicker_handler.take() {
            self.kicker.disconnect(id);
        }
        match kicker {
            Some(target) => {
                self.kicker.set_label(&target.name);
                self.kicker.set_visible(true);
                let weak = ui.downgrade();
                let id = self.kicker.connect_clicked(move |_| {
                    if let Some(ui) = weak.upgrade() {
                        ui.open(&target);
                    }
                });
                self.kicker_handler.replace(Some(id));
            }
            None => self.kicker.set_visible(false),
        }
    }
}

impl Hero {
    fn show_facts(&self, item: &BaseItem) {
        while let Some(child) = self.facts.first_child() {
            self.facts.remove(&child);
        }
        let facts = facts(item);
        for (row, (name, value)) in facts.iter().enumerate() {
            let row = row as i32;
            let name = gtk::Label::builder()
                .label(*name)
                .xalign(0.0)
                .yalign(0.0)
                .css_classes(["dim-label"])
                .build();
            let value = gtk::Label::builder()
                .label(super::plain_spaces(value))
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .hexpand(true)
                .build();
            self.facts.attach(&name, 0, row, 1, 1);
            self.facts.attach(&value, 1, row, 1, 1);
        }
        self.facts.set_visible(!facts.is_empty());
    }
}

/// Lines of overview an episode page keeps room for (longer ones still
/// show in full).
const EPISODE_OVERVIEW_LINES: i32 = 4;

/// Tags shown before "and N more" (anime databases list dozens).
const TAG_LIMIT: usize = 12;

/// The details rows for `item`, leaving out whatever the server doesn't
/// have.
pub fn facts(item: &BaseItem) -> Vec<(&'static str, String)> {
    let mut facts = Vec::new();
    if let Some(original) = item
        .original_title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != item.name)
    {
        facts.push(("Original title", original.to_string()));
    }
    match item.item_type.as_str() {
        // In the meta line already.
        "Episode" => {}
        "Series" | "Season" => {
            if let Some(range) = aired_range(item) {
                facts.push(("Aired", range));
            }
        }
        _ => {
            if let Some(date) = item.premiere_date.as_deref().and_then(super::format_date) {
                facts.push(("Released", date));
            }
        }
    }
    if let Some(status) = item
        .status
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        facts.push(("Status", status.to_string()));
    }
    let names = |list: &[crate::emby::models::NameId]| {
        list.iter()
            .map(|n| n.name.trim())
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let genres = names(&item.genre_items);
    if !genres.is_empty() {
        facts.push(("Genres", genres));
    }
    let studios = names(&item.studios);
    if !studios.is_empty() {
        facts.push(("Studios", studios));
    }
    let places = item
        .production_locations
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>();
    if !places.is_empty() {
        facts.push(("Country", places.join(", ")));
    }
    let tags = item
        .tags
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>();
    if !tags.is_empty() {
        let mut text = tags[..tags.len().min(TAG_LIMIT)].join(", ");
        if tags.len() > TAG_LIMIT {
            text.push_str(&format!(" and {} more", tags.len() - TAG_LIMIT));
        }
        facts.push(("Tags", text));
    }
    facts
}

/// When a series or season aired: "4 Apr 2015 – 20 Mar 2025", or just its
/// start while it hasn't ended (or the end isn't known).
pub fn aired_range(item: &BaseItem) -> Option<String> {
    // An end before the start is bad metadata: neither date can be trusted.
    if let (Some(start), Some(end)) = (&item.premiere_date, &item.end_date)
        && end.get(..10) < start.get(..10)
    {
        return None;
    }
    let start = item.premiere_date.as_deref().and_then(super::format_date);
    let end = item.end_date.as_deref().and_then(super::format_date);
    match (start, end) {
        (Some(start), Some(end)) if start != end => Some(format!("{start} – {end}")),
        (Some(start), _) => Some(start),
        (None, Some(end)) => Some(format!("until {end}")),
        (None, None) => None,
    }
}

const SIMILAR_LIMIT: usize = 16;
const CAST_LIMIT: usize = 30;

/// "Cast & Crew" and "More Like This" rows under a title, into `related`.
pub fn show_related(ui: &Ui, related: &gtk::Box, item: &BaseItem) {
    // The rows are swapped in all at once, when everything has arrived:
    // clearing first shortened the page for a moment and it scrolled.
    // SAFETY: this key is only ever stored and read as `String`.
    unsafe { related.set_data(RELATED_KEY, item.id.clone()) };
    let people: Vec<BaseItem> = item
        .people
        .iter()
        .take(CAST_LIMIT)
        .map(|person| person.as_item())
        .collect();
    let client = ui.client();
    let user_id = ui.user_id();
    let item_id = item.id.clone();
    let wanted = item.id.clone();
    let weak = ui.downgrade();
    let related = related.downgrade();
    glib::spawn_future_local(async move {
        // "Included In": the collections holding it, as Emby web shows.
        let collections = ItemQuery {
            list_item_id: Some(item_id.clone()),
            include_types: Some("BoxSet"),
            recursive: true,
            limit: SIMILAR_LIMIT,
            ..Default::default()
        };
        let (included, similar) = spawn_tokio(async move {
            tokio::join!(
                client.items(&user_id, &collections),
                client.similar(&user_id, &item_id, SIMILAR_LIMIT)
            )
        })
        .await;
        let (Some(ui), Some(related)) = (weak.upgrade(), related.upgrade()) else {
            return;
        };
        // SAFETY: see above. Another title may have been shown meanwhile.
        let current = unsafe {
            related
                .data::<String>(RELATED_KEY)
                .is_some_and(|id| *id.as_ref() == wanted)
        };
        if !current {
            return;
        }
        let mark = super::gamepad::mark_cursor(&related);
        super::clear(&related);
        if !people.is_empty() {
            related.append(&row(
                &ui,
                "Cast & Crew",
                &people,
                Shape::Person,
                Click::Open,
                More::None,
            ));
        }
        match included {
            Ok(result) if !result.items.is_empty() => {
                related.append(&row(
                    &ui,
                    "Included In",
                    &result.items,
                    Shape::Poster,
                    Click::Open,
                    More::None,
                ));
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("collections holding the item failed: {e:#}"),
        }
        match similar {
            Ok(items) if !items.is_empty() => {
                related.append(&row(
                    &ui,
                    "More Like This",
                    &items,
                    Shape::Poster,
                    Click::Open,
                    More::None,
                ));
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("similar items failed: {e:#}"),
        }
        if let Some(mark) = mark {
            mark.restore();
        }
    });
}

const RELATED_KEY: &str = "embyclientplus-related-for";

/// Watched and favourite toggles for the hero's button row.
pub fn item_toggles(ui: &Ui, item: &BaseItem) -> [gtk::Button; 2] {
    let played = item.played();
    let watched = gtk::Button::builder()
        .icon_name(crate::ui::icons::WATCHED)
        .tooltip_text(if played {
            "Mark as unwatched"
        } else {
            "Mark as watched"
        })
        .valign(gtk::Align::Center)
        .css_classes(if played {
            vec!["circular", "accent"]
        } else {
            vec!["circular"]
        })
        .build();
    let favorite = item.is_favorite();
    let star = gtk::Button::builder()
        .icon_name(if favorite {
            crate::ui::icons::FAVORITE
        } else {
            crate::ui::icons::NOT_FAVORITE
        })
        .tooltip_text(if favorite {
            "Remove from favourites"
        } else {
            "Add to favourites"
        })
        .valign(gtk::Align::Center)
        .css_classes(if favorite {
            vec!["circular", "accent"]
        } else {
            vec!["circular"]
        })
        .build();
    let weak = ui.downgrade();
    let target = item.clone();
    watched.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.set_played(&target, !played);
        }
    });
    let weak = ui.downgrade();
    let target = item.clone();
    star.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.set_favorite(&target, !favorite);
        }
    });
    [watched, star]
}

/// "2024 · 24 min · PG-13 · ★ 7.8 · Watched"
pub fn meta_line(item: &BaseItem) -> String {
    let mut parts = Vec::new();
    if let Some(year) = item.production_year {
        parts.push(year.to_string());
    }
    if let Some(ticks) = item.run_time_ticks.filter(|_| item.item_type != "Series") {
        parts.push(format_runtime(ticks));
    }
    if let Some(rating) = &item.official_rating {
        parts.push(rating.clone());
    }
    if let Some(score) = item.community_rating {
        parts.push(format!("★ {score:.1}"));
    }
    if let Some(score) = item.critic_rating.filter(|s| *s > 0.0) {
        parts.push(format!("Critics {score:.0}%"));
    }
    if item.item_type == "Episode"
        && let Some(aired) = item.premiere_date.as_deref().and_then(super::format_date)
    {
        parts.push(format!("Aired {aired}"));
    }
    if let Some(added) = item.date_created.as_deref().and_then(super::format_date) {
        parts.push(format!("Added {added}"));
    }
    if item.played() {
        parts.push("Watched".to_string());
    } else if let Some(unplayed) = item
        .user_data
        .as_ref()
        .and_then(|data| data.unplayed_item_count)
        .filter(|&n| n > 0 && item.item_type == "Series")
    {
        parts.push(format!("{unplayed} unwatched"));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emby::models::UserItemData;

    #[test]
    fn meta_line_joins_available_parts() {
        let item = BaseItem {
            production_year: Some(2024),
            run_time_ticks: Some(24 * 60 * crate::playback::TICKS_PER_SECOND),
            community_rating: Some(7.84),
            ..Default::default()
        };
        assert_eq!(meta_line(&item), "2024 · 24 min · ★ 7.8");
    }

    #[test]
    fn facts_leave_out_what_is_missing() {
        assert!(facts(&BaseItem::default()).is_empty());
        let series = BaseItem {
            name: "DanMachi".into(),
            item_type: "Series".into(),
            original_title: Some("ダンジョンに出会いを求めるのは間違っているだろうか".into()),
            premiere_date: Some("2015-04-04T00:00:00.0000000Z".into()),
            end_date: Some("2025-03-20T00:00:00.0000000Z".into()),
            status: Some("Ended".into()),
            studios: vec![crate::emby::models::NameId {
                name: "J.C.Staff".into(),
                id: "1".into(),
            }],
            tags: (1..=14).map(|n| format!("tag{n}")).collect(),
            production_locations: vec![" ".into()],
            ..Default::default()
        };
        let facts = facts(&series);
        let names: Vec<&str> = facts.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            ["Original title", "Aired", "Status", "Studios", "Tags"]
        );
        assert_eq!(facts[1].1, "4 Apr 2015 – 20 Mar 2025");
        assert!(facts[4].1.ends_with("tag12 and 2 more"));
    }

    #[test]
    fn original_title_same_as_name_is_left_out() {
        let movie = BaseItem {
            name: "Akira".into(),
            item_type: "Movie".into(),
            original_title: Some("Akira".into()),
            premiere_date: Some("1988-07-16T00:00:00Z".into()),
            ..Default::default()
        };
        assert_eq!(facts(&movie), [("Released", "16 Jul 1988".to_string())]);
    }

    #[test]
    fn a_range_ending_before_it_starts_is_left_out() {
        let season = BaseItem {
            premiere_date: Some("2026-01-02T00:00:00Z".into()),
            end_date: Some("1995-01-28T00:00:00Z".into()),
            ..Default::default()
        };
        assert_eq!(aired_range(&season), None);
    }

    #[test]
    fn series_meta_counts_unwatched_instead_of_runtime() {
        let series = BaseItem {
            item_type: "Series".into(),
            production_year: Some(2026),
            run_time_ticks: Some(1),
            user_data: Some(UserItemData {
                unplayed_item_count: Some(5),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(meta_line(&series), "2026 · 5 unwatched");
    }
}
