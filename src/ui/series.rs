//! Series overview: backdrop, season picker, episode list.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::{Ui, format_runtime, images, loading, reload_after_playback, scrolled_page};
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

struct SeriesView {
    backdrop: gtk::Picture,
    title: gtk::Label,
    overview: gtk::Label,
    seasons: gtk::DropDown,
    episodes: gtk::ListBox,
    /// Seasons backing the dropdown, in its order.
    season_items: RefCell<Vec<BaseItem>>,
}

pub fn page(ui: &Ui, series: &BaseItem) -> adw::NavigationPage {
    let backdrop = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .height_request(280)
        .css_classes(["backdrop"])
        .build();
    let title = gtk::Label::builder()
        .label(&series.name)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-1"])
        .build();
    let overview = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(["body"])
        .build();
    let seasons = gtk::DropDown::builder().halign(gtk::Align::Start).build();
    let episodes = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    episodes.append(&loading());

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .build();
    body.append(&title);
    body.append(&overview);
    body.append(&seasons);
    body.append(&episodes);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&backdrop);
    content.append(
        &adw::Clamp::builder()
            .maximum_size(960)
            .margin_top(18)
            .margin_bottom(24)
            .margin_start(18)
            .margin_end(18)
            .child(&body)
            .build(),
    );

    let header = adw::HeaderBar::new();
    let page = scrolled_page(&series.name, None, &header, &content);
    let view = Rc::new(SeriesView {
        backdrop,
        title,
        overview,
        seasons,
        episodes,
        season_items: RefCell::new(Vec::new()),
    });

    let weak = ui.downgrade();
    let series_id = series.id.clone();
    view.seasons.connect_selected_notify({
        let view = view.clone();
        let series_id = series_id.clone();
        move |dropdown| {
            let Some(ui) = weak.upgrade() else { return };
            let season = view
                .season_items
                .borrow()
                .get(dropdown.selected() as usize)
                .cloned();
            if let Some(season) = season {
                load_episodes(&ui, &view, &series_id, &season.id);
            }
        }
    });

    reload_after_playback(ui, &page, {
        let weak = ui.downgrade();
        let view = view.clone();
        let series_id = series_id.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                load(&ui, &view, &series_id);
            }
        }
    });
    load(ui, &view, &series_id);
    page
}

fn load(ui: &Ui, view: &Rc<SeriesView>, series_id: &str) {
    let client = ui.client();
    let user_id = ui.user_id();
    let series_id = series_id.to_string();
    let ui = ui.clone();
    let view = view.clone();
    glib::spawn_future_local(async move {
        let result = spawn_tokio({
            let series_id = series_id.clone();
            async move {
                let (series, seasons) = tokio::join!(
                    client.item(&user_id, &series_id),
                    client.seasons(&series_id, &user_id),
                );
                Ok::<_, anyhow::Error>((series?, seasons?))
            }
        })
        .await;
        let (series, seasons) = match result {
            Ok(data) => data,
            Err(e) => {
                clear_list(&view.episodes);
                ui.report_error("Could not load series", &e);
                return;
            }
        };

        view.title.set_label(&series.name);
        let overview = series.overview.clone().unwrap_or_default();
        view.overview.set_label(&overview);
        view.overview.set_visible(!overview.is_empty());
        images::load(&ui, &view.backdrop, series.backdrop(), 1920);

        // Keep the user's current season across reloads; otherwise start at
        // the first one with something left to watch.
        let previous = view
            .season_items
            .borrow()
            .get(view.seasons.selected() as usize)
            .map(|season| season.id.clone());
        let selected = previous
            .and_then(|id| seasons.iter().position(|season| season.id == id))
            .unwrap_or_else(|| default_season(&seasons));
        let ids = |items: &[BaseItem]| items.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        let same_list = ids(&view.season_items.borrow()) == ids(&seasons);
        view.season_items.replace(seasons.clone());
        view.seasons.set_visible(seasons.len() > 1);
        if !same_list {
            let names: Vec<&str> = seasons.iter().map(|season| season.name.as_str()).collect();
            view.seasons.set_model(Some(&gtk::StringList::new(&names)));
        }
        if seasons.get(selected).is_none() {
            clear_list(&view.episodes);
        } else if view.seasons.selected() as usize == selected {
            // No selection change, so the notify handler won't load it.
            load_episodes(&ui, &view, &series_id, &seasons[selected].id);
        } else {
            view.seasons.set_selected(selected as u32);
        }
    });
}

fn default_season(seasons: &[BaseItem]) -> usize {
    seasons
        .iter()
        .position(|season| {
            season
                .user_data
                .as_ref()
                .and_then(|data| data.unplayed_item_count)
                .is_some_and(|count| count > 0)
        })
        .unwrap_or(0)
}

fn load_episodes(ui: &Ui, view: &Rc<SeriesView>, series_id: &str, season_id: &str) {
    let client = ui.client();
    let user_id = ui.user_id();
    let series_id = series_id.to_string();
    let season_id = season_id.to_string();
    let ui = ui.clone();
    let view = view.clone();
    glib::spawn_future_local(async move {
        let wanted = season_id.clone();
        let result =
            spawn_tokio(async move { client.episodes(&series_id, &season_id, &user_id).await })
                .await;
        // The user may have switched seasons while this was loading.
        let current = view
            .season_items
            .borrow()
            .get(view.seasons.selected() as usize)
            .map(|season| season.id.clone());
        if current.as_deref() != Some(wanted.as_str()) {
            return;
        }
        clear_list(&view.episodes);
        match result {
            Ok(episodes) => {
                for episode in &episodes {
                    view.episodes.append(&episode_row(&ui, episode));
                }
            }
            Err(e) => ui.report_error("Could not load episodes", &e),
        }
    });
}

fn episode_row(ui: &Ui, episode: &BaseItem) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&episode.episode_label()))
        .activatable(true)
        .build();
    let mut details = Vec::new();
    if let Some(ticks) = episode.run_time_ticks {
        details.push(format_runtime(ticks));
    }
    if let (Some(fraction), Some(ticks)) = (episode.progress(), episode.run_time_ticks) {
        let remaining = (ticks as f64 * (1.0 - fraction)) as i64;
        details.push(format!("{} left", format_runtime(remaining)));
    }
    row.set_subtitle(&glib::markup_escape_text(&details.join(" · ")));

    let thumb = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .width_request(160)
        .height_request(90)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    let frame = gtk::Overlay::builder()
        .child(&thumb)
        .overflow(gtk::Overflow::Hidden)
        .valign(gtk::Align::Center)
        .css_classes(["card"])
        .build();
    if let Some(fraction) = episode.progress() {
        frame.add_overlay(
            &gtk::ProgressBar::builder()
                .fraction(fraction)
                .valign(gtk::Align::End)
                .margin_start(6)
                .margin_end(6)
                .margin_bottom(10)
                .build(),
        );
    }
    images::load(ui, &thumb, episode.landscape(), 320);
    row.add_prefix(&frame);
    if episode.played() {
        row.add_suffix(&gtk::Image::from_icon_name("object-select-symbolic"));
    }
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));

    let weak = ui.downgrade();
    let target = episode.clone();
    row.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.open(&target);
        }
    });
    row
}

fn clear_list(list: &gtk::ListBox) {
    list.remove_all();
}
