//! Home: libraries, Continue Watching, Next Up, and Latest per library.

use adw::prelude::*;
use gtk::{gio, glib};

use super::card::Shape;
use super::rows::{Click, More, row};
use super::{Ui, clear, loading, reload_on_show, scrolled_page};
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

const ROW_LIMIT: usize = 20;

/// Libraries whose Latest row is worth showing.
fn has_latest_row(view: &BaseItem) -> bool {
    matches!(
        view.collection_type.as_deref(),
        Some("movies" | "tvshows" | "homevideos" | "musicvideos" | "music") | None
    )
}

struct HomeData {
    views: Vec<BaseItem>,
    resume: Vec<BaseItem>,
    next_up: Vec<BaseItem>,
    /// Each library's view, with its newest items.
    latest: Vec<(BaseItem, Vec<BaseItem>)>,
}

pub fn page(ui: &Ui) -> adw::NavigationPage {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .margin_top(12)
        .margin_bottom(24)
        .build();

    let header = adw::HeaderBar::new();
    let search = gtk::Button::builder()
        .icon_name(crate::ui::icons::SEARCH)
        .tooltip_text("Search")
        .build();
    header.pack_start(&search);
    let favorites = gtk::Button::builder()
        .icon_name(crate::ui::icons::FAVORITE)
        .tooltip_text("Favorites")
        .build();
    header.pack_start(&favorites);
    let weak = ui.downgrade();
    favorites.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.open_favorites();
        }
    });
    let menu = gio::Menu::new();
    menu.append(Some("Refresh"), Some("home.refresh"));
    menu.append(Some("Preferences"), Some("home.preferences"));
    menu.append(Some("Log Out"), Some("home.logout"));
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name(crate::ui::icons::MENU)
            .menu_model(&menu)
            .build(),
    );

    let page = scrolled_page("Home", Some("home"), &header, &content);

    let weak = ui.downgrade();
    search.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.open_search();
        }
    });

    let actions = gio::SimpleActionGroup::new();
    let refresh = gio::SimpleAction::new("refresh", None);
    refresh.connect_activate({
        let weak = ui.downgrade();
        let content = content.clone();
        move |_, _| {
            if let Some(ui) = weak.upgrade() {
                load(&ui, &content);
            }
        }
    });
    let logout = gio::SimpleAction::new("logout", None);
    logout.connect_activate({
        let weak = ui.downgrade();
        move |_, _| {
            if let Some(ui) = weak.upgrade() {
                ui.logout();
            }
        }
    });
    actions.add_action(&refresh);
    let preferences = gio::SimpleAction::new("preferences", None);
    preferences.connect_activate(glib::clone!(
        #[weak]
        content,
        move |_, _| super::preferences::show(&content)
    ));
    actions.add_action(&preferences);
    actions.add_action(&logout);
    page.insert_action_group("home", Some(&actions));

    // Home always reloads when you come back to it.
    reload_on_show(ui, &page, {
        let weak = ui.downgrade();
        let content = content.clone();
        move || {
            if let Some(ui) = weak.upgrade() {
                load(&ui, &content);
            }
        }
    });

    content.append(&loading());
    load(ui, &content);
    page
}

fn load(ui: &Ui, content: &gtk::Box) {
    let client = ui.client();
    let user_id = ui.user_id();
    let ui = ui.clone();
    let content = content.clone();
    glib::spawn_future_local(async move {
        let result = spawn_tokio(async move {
            let (views, resume, next_up) = tokio::join!(
                client.views(&user_id),
                client.resume(&user_id, None, ROW_LIMIT),
                client.next_up(&user_id, None, ROW_LIMIT),
            );
            let views = views?;
            let mut latest_tasks = tokio::task::JoinSet::new();
            for (index, view) in views.iter().filter(|v| has_latest_row(v)).enumerate() {
                let client = client.clone();
                let user_id = user_id.clone();
                let view = view.clone();
                latest_tasks.spawn(async move {
                    let items = client.latest(&user_id, &view.id, ROW_LIMIT).await;
                    (index, view, items)
                });
            }
            let mut latest = Vec::new();
            while let Some(joined) = latest_tasks.join_next().await {
                let (index, view, items) = joined?;
                match items {
                    Ok(items) if !items.is_empty() => latest.push((index, view, items)),
                    Ok(_) => {}
                    Err(e) => tracing::warn!("latest in {} failed: {e:#}", view.name),
                }
            }
            latest.sort_by_key(|(index, _, _)| *index);
            Ok::<_, anyhow::Error>(HomeData {
                views,
                // A failed secondary row shouldn't blank the whole page.
                resume: resume.unwrap_or_else(|e| {
                    tracing::warn!("resume row failed: {e:#}");
                    Vec::new()
                }),
                next_up: next_up.unwrap_or_else(|e| {
                    tracing::warn!("next up row failed: {e:#}");
                    Vec::new()
                }),
                latest: latest
                    .into_iter()
                    .map(|(_, view, items)| (view, items))
                    .collect(),
            })
        })
        .await;

        match result {
            Ok(data) => show(&ui, &content, data),
            Err(e) => {
                clear(&content);
                ui.report_error("Could not load home", &e);
            }
        }
    });
}

fn show(ui: &Ui, content: &gtk::Box, data: HomeData) {
    clear(content);
    if !data.views.is_empty() {
        content.append(&row(
            ui,
            "Libraries",
            &data.views,
            Shape::Landscape,
            Click::Open,
            More::None,
        ));
    }
    if !data.resume.is_empty() {
        content.append(&row(
            ui,
            "Continue Watching",
            &data.resume,
            Shape::for_episodes(),
            Click::Resume,
            More::Resume(None),
        ));
    }
    if !data.next_up.is_empty() {
        content.append(&row(
            ui,
            "Next Up",
            &data.next_up,
            Shape::for_episodes(),
            Click::Open,
            More::NextUp(None),
        ));
    }
    for (library, items) in &data.latest {
        content.append(&row(
            ui,
            &format!("Latest in {}", library.name),
            items,
            if library.collection_type.as_deref() == Some("music") {
                Shape::Square
            } else {
                Shape::Poster
            },
            Click::Open,
            More::Library(Box::new(library.clone())),
        ));
    }
}
