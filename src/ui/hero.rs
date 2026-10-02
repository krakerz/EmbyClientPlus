//! The header shared by series and details pages: fan-art banner, then
//! poster beside title, meta line, buttons and overview. `extra` holds
//! whatever the page shows below (episodes, seasons).

use std::cell::RefCell;

use adw::prelude::*;
use gtk::glib;

use super::card::Shape;
use super::rows::{Click, More, row};
use super::{Ui, banner, fixed_picture, format_runtime, images};
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
    poster_frame: gtk::Overlay,
    still: gtk::Picture,
    still_frame: gtk::Overlay,
    /// Link above the title (the series, on an episode page).
    kicker: gtk::Button,
    kicker_handler: RefCell<Option<glib::SignalHandlerId>>,
    title: gtk::Label,
    meta: gtk::Label,
    pub buttons: gtk::Box,
    overview: gtk::Label,
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
        let overview = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["body"])
            .build();

        let info = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .hexpand(true)
            .build();
        info.append(&kicker);
        info.append(&title);
        info.append(&meta);
        info.append(&buttons);
        info.append(&overview);
        let top = gtk::Box::builder().spacing(24).build();
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
                .maximum_size(1080)
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
            poster_frame,
            still,
            still_frame,
            kicker,
            kicker_handler: RefCell::new(None),
            title,
            meta,
            buttons,
            overview,
            extra,
        }
    }

    /// Fills in `item`; `kicker` is a page to link above the title.
    pub fn show(&self, ui: &Ui, item: &BaseItem, kicker: Option<BaseItem>) {
        self.title.set_label(&item.episode_label());
        self.meta.set_label(&meta_line(item));
        let overview = item.overview.clone().unwrap_or_default();
        self.overview.set_label(&overview);
        self.overview.set_visible(!overview.is_empty());
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

const SIMILAR_LIMIT: usize = 16;
const CAST_LIMIT: usize = 30;

/// "Cast & Crew" and "More Like This" rows under a title, into `related`.
pub fn show_related(ui: &Ui, related: &gtk::Box, item: &BaseItem) {
    super::clear(related);
    let people: Vec<BaseItem> = item
        .people
        .iter()
        .take(CAST_LIMIT)
        .map(|person| person.as_item())
        .collect();
    if !people.is_empty() {
        related.append(&row(
            ui,
            "Cast & Crew",
            &people,
            Shape::Person,
            Click::Open,
            More::None,
        ));
    }
    let client = ui.client();
    let user_id = ui.user_id();
    let item_id = item.id.clone();
    let weak = ui.downgrade();
    let related = related.downgrade();
    glib::spawn_future_local(async move {
        let similar =
            spawn_tokio(async move { client.similar(&user_id, &item_id, SIMILAR_LIMIT).await })
                .await;
        let (Some(ui), Some(related)) = (weak.upgrade(), related.upgrade()) else {
            return;
        };
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
    });
}

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
