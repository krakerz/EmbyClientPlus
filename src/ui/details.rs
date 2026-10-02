//! Movie/episode details with Resume / Play buttons.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::{
    Ui, clear, format_runtime, format_timestamp, images, reload_after_playback, scrolled_page,
};
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

struct DetailsView {
    backdrop: gtk::Picture,
    poster: gtk::Picture,
    series: gtk::Button,
    /// The series button's click handler; replaced on each refresh.
    series_handler: RefCell<Option<glib::SignalHandlerId>>,
    title: gtk::Label,
    meta: gtk::Label,
    overview: gtk::Label,
    buttons: gtk::Box,
}

pub fn page(ui: &Ui, item: &BaseItem) -> adw::NavigationPage {
    let backdrop = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .height_request(320)
        .build();
    let poster = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .width_request(180)
        .height_request(270)
        .build();
    let poster_frame = gtk::Overlay::builder()
        .child(&poster)
        .overflow(gtk::Overflow::Hidden)
        .valign(gtk::Align::Start)
        .css_classes(["card"])
        .build();
    let series = gtk::Button::builder()
        .halign(gtk::Align::Start)
        .css_classes(["flat", "heading"])
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
    info.append(&series);
    info.append(&title);
    info.append(&meta);
    info.append(&buttons);
    info.append(&overview);
    let body = gtk::Box::builder().spacing(24).build();
    body.append(&poster_frame);
    body.append(&info);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&backdrop);
    content.append(
        &adw::Clamp::builder()
            .maximum_size(1080)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(18)
            .margin_end(18)
            .child(&body)
            .build(),
    );
    let header = adw::HeaderBar::new();
    let page = scrolled_page(&item.name, None, &header, &content);

    let view = Rc::new(DetailsView {
        backdrop,
        poster,
        series,
        series_handler: RefCell::new(None),
        title,
        meta,
        overview,
        buttons,
    });
    // Show what we already have right away, then refresh from the server
    // (list results can lack the overview, and watch state may be stale).
    show(ui, &view, item);
    let item_id = item.id.clone();
    reload_after_playback(ui, &page, {
        let weak = ui.downgrade();
        let view = view.clone();
        let item_id = item_id.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                load(&ui, &view, &item_id);
            }
        }
    });
    load(ui, &view, &item_id);
    page
}

fn load(ui: &Ui, view: &Rc<DetailsView>, item_id: &str) {
    let client = ui.client();
    let user_id = ui.user_id();
    let item_id = item_id.to_string();
    let ui = ui.clone();
    let view = view.clone();
    glib::spawn_future_local(async move {
        match spawn_tokio(async move { client.item(&user_id, &item_id).await }).await {
            Ok(item) => show(&ui, &view, &item),
            Err(e) => ui.report_error("Could not load details", &e),
        }
    });
}

fn show(ui: &Ui, view: &DetailsView, item: &BaseItem) {
    view.title.set_label(&item.episode_label());
    view.meta.set_label(&meta_line(item));
    let overview = item.overview.clone().unwrap_or_default();
    view.overview.set_label(&overview);
    view.overview.set_visible(!overview.is_empty());
    images::load(ui, &view.backdrop, item.backdrop(), 1920);
    images::load(ui, &view.poster, item.poster(), 360);

    match (&item.series_name, &item.series_id) {
        (Some(name), Some(series_id)) => {
            view.series.set_label(name);
            view.series.set_visible(true);
            let series = BaseItem {
                id: series_id.clone(),
                name: name.clone(),
                item_type: "Series".into(),
                ..Default::default()
            };
            let weak = ui.downgrade();
            if let Some(id) = view.series_handler.take() {
                view.series.disconnect(id);
            }
            let id = view.series.connect_clicked(move |_| {
                if let Some(ui) = weak.upgrade() {
                    ui.open(&series);
                }
            });
            view.series_handler.replace(Some(id));
        }
        _ => view.series.set_visible(false),
    }

    clear(&view.buttons);
    let resume_ticks = item.resume_ticks();
    if resume_ticks > 0 {
        view.buttons.append(&play_button(
            ui,
            item,
            &format!("Resume {}", format_timestamp(resume_ticks)),
            resume_ticks,
            true,
        ));
        view.buttons
            .append(&play_button(ui, item, "Play from Beginning", 0, false));
    } else {
        view.buttons.append(&play_button(ui, item, "Play", 0, true));
    }
}

fn play_button(
    ui: &Ui,
    item: &BaseItem,
    label: &str,
    start_ticks: i64,
    primary: bool,
) -> gtk::Button {
    let button = gtk::Button::builder()
        .label(label)
        .css_classes(if primary {
            vec!["suggested-action", "pill"]
        } else {
            vec!["pill"]
        })
        .build();
    let weak = ui.downgrade();
    let item = item.clone();
    button.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.play(&item, start_ticks);
        }
    });
    button
}

/// "2024 · 24 min · PG-13 · ★ 7.8"
fn meta_line(item: &BaseItem) -> String {
    let mut parts = Vec::new();
    if let Some(year) = item.production_year {
        parts.push(year.to_string());
    }
    if let Some(ticks) = item.run_time_ticks {
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
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
