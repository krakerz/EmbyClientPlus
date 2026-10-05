//! History: what was played, newest first, grouped by day.

use adw::prelude::*;
use gtk::glib;

use super::{Ui, clear, fixed_picture, format_runtime, images, loading};
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

/// How far back the page goes.
const LIMIT: usize = 200;

pub fn page(ui: &Ui) -> adw::NavigationPage {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&loading());
    let clamp = adw::Clamp::builder()
        .maximum_size(1000)
        .child(&content)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .vexpand(true)
        .build();
    let header = adw::HeaderBar::new();
    super::add_home_button(&header);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scrolled));
    let page = adw::NavigationPage::new(&toolbar, "History");

    super::reload_on_change(ui, &page, {
        let (weak, content) = (ui.downgrade(), content.clone());
        move || {
            if let Some(ui) = weak.upgrade() {
                load(&ui, &content);
            }
        }
    });
    load(ui, &content);
    page
}

fn load(ui: &Ui, content: &gtk::Box) {
    let (client, user_id) = (ui.client(), ui.user_id());
    let (weak, content) = (ui.downgrade(), content.clone());
    glib::spawn_future_local(async move {
        let result = spawn_tokio(async move { client.history(&user_id, LIMIT).await }).await;
        let Some(ui) = weak.upgrade() else { return };
        clear(&content);
        let items = match result {
            Ok(items) => items,
            Err(e) => {
                ui.report_error("Could not load the history", &e);
                return;
            }
        };
        if items.is_empty() {
            content.append(
                &adw::StatusPage::builder()
                    .icon_name(crate::ui::icons::HISTORY)
                    .title("Nothing played yet")
                    .description("What you watch shows up here, newest first")
                    .vexpand(true)
                    .build(),
            );
            return;
        }
        let today = glib::DateTime::now_local().ok();
        let mut current_day: Option<String> = None;
        let mut list: Option<gtk::ListBox> = None;
        for item in &items {
            let played = played_at(item);
            let day = played
                .as_ref()
                .map(|time| day_label(time, today.as_ref()))
                .unwrap_or_default();
            if current_day.as_deref() != Some(day.as_str()) || list.is_none() {
                content.append(
                    &gtk::Label::builder()
                        .label(&day)
                        .xalign(0.0)
                        .css_classes(["heading"])
                        .build(),
                );
                let new_list = gtk::ListBox::builder()
                    .selection_mode(gtk::SelectionMode::None)
                    .css_classes(["boxed-list"])
                    .build();
                content.append(&new_list);
                list = Some(new_list);
                current_day = Some(day);
            }
            if let Some(list) = &list {
                list.append(&row(&ui, item, played.as_ref()));
            }
        }
    });
}

/// When `item` was last played, in local time.
fn played_at(item: &BaseItem) -> Option<glib::DateTime> {
    let text = item.user_data.as_ref()?.last_played_date.as_deref()?;
    glib::DateTime::from_iso8601(text, None)
        .ok()?
        .to_local()
        .ok()
}

/// "Today", "Yesterday", else "Friday, 3 Oct 2026".
fn day_label(time: &glib::DateTime, today: Option<&glib::DateTime>) -> String {
    let same_day = |a: &glib::DateTime, b: &glib::DateTime| {
        a.year() == b.year() && a.day_of_year() == b.day_of_year()
    };
    if let Some(today) = today {
        if same_day(time, today) {
            return "Today".into();
        }
        if let Ok(yesterday) = today.add_days(-1)
            && same_day(time, &yesterday)
        {
            return "Yesterday".into();
        }
    }
    time.format("%A, %-d %b %Y")
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn row(ui: &Ui, item: &BaseItem, played: Option<&glib::DateTime>) -> adw::ActionRow {
    let title = match (&item.series_name, item.item_type.as_str()) {
        (Some(series), "Episode") => format!("{series} · {}", item.episode_label()),
        _ => item.name.clone(),
    };
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&title))
        .activatable(true)
        .build();
    let mut details = Vec::new();
    if let Some(time) = played.and_then(|t| t.format("%H:%M").ok()) {
        details.push(format!("Played at {time}"));
    }
    if let Some(ticks) = item.run_time_ticks {
        details.push(format_runtime(ticks));
    }
    if let (Some(fraction), Some(ticks)) = (item.progress(), item.run_time_ticks) {
        let remaining = (ticks as f64 * (1.0 - fraction)) as i64;
        details.push(format!("{} left", format_runtime(remaining)));
    } else if item.played() {
        details.push("Watched".into());
    }
    row.set_subtitle(&glib::markup_escape_text(&details.join(" · ")));

    let thumb = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .build();
    let frame = gtk::Overlay::builder()
        .child(&fixed_picture(&thumb, 160, 90))
        .overflow(gtk::Overflow::Hidden)
        .valign(gtk::Align::Center)
        .margin_top(6)
        .margin_bottom(6)
        .css_classes(["card"])
        .build();
    if let Some(fraction) = item.progress() {
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
    images::load(ui, &thumb, item.landscape().or_else(|| item.poster()), 320);
    row.add_prefix(&frame);
    row.add_suffix(&gtk::Image::from_icon_name(crate::ui::icons::NEXT));
    let (weak, target) = (ui.downgrade(), item.clone());
    row.connect_activated(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.open(&target);
        }
    });
    row
}
