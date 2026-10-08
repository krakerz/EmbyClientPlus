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
    // Search starts right here in the header; Enter (or the search button)
    // opens the results page.
    let search_entry = gtk::SearchEntry::builder()
        .placeholder_text("Search movies, shows, episodes")
        .width_request(320)
        .build();
    let search_bar = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideRight)
        .child(&search_entry)
        .build();
    header.pack_start(&search_bar);
    let weak = ui.downgrade();
    favorites.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            ui.open_favorites();
        }
    });
    let menu = gio::Menu::new();
    menu.append(Some("Refresh"), Some("home.refresh"));
    menu.append(Some("History"), Some("home.history"));
    menu.append(Some("Downloads"), Some("home.downloads"));
    menu.append(Some("Preferences"), Some("home.preferences"));
    menu.append(Some("Log Out"), Some("home.logout"));
    // Easier with a controller than Steam's own menu.
    menu.append(Some("Quit"), Some("home.quit"));
    header.pack_end(
        &gtk::MenuButton::builder()
            .icon_name(crate::ui::icons::MENU)
            .menu_model(&menu)
            .build(),
    );
    header.pack_end(&ui_scale_buttons(ui));

    let page = scrolled_page("Home", Some("home"), &header, &content);

    let submit = {
        let (weak, entry, bar) = (ui.downgrade(), search_entry.clone(), search_bar.clone());
        move || {
            let term = entry.text().trim().to_string();
            if term.is_empty() {
                return false;
            }
            if let Some(ui) = weak.upgrade() {
                ui.open_search(&term);
            }
            entry.set_text("");
            bar.set_reveal_child(false);
            true
        }
    };
    let show_search = {
        let (entry, bar) = (search_entry.clone(), search_bar.clone());
        move || {
            bar.set_reveal_child(true);
            entry.grab_focus();
        }
    };
    search.connect_clicked({
        let (submit, show_search, bar) = (submit.clone(), show_search.clone(), search_bar.clone());
        move |_| {
            if !bar.reveals_child() {
                show_search();
            } else if !submit() {
                bar.set_reveal_child(false);
            }
        }
    });
    search_entry.connect_activate(move |_| {
        submit();
    });
    search_entry.connect_stop_search({
        let bar = search_bar.clone();
        move |entry| {
            entry.set_text("");
            bar.set_reveal_child(false);
        }
    });
    ui.set_search_starter(show_search);

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
    let history = gio::SimpleAction::new("history", None);
    history.connect_activate({
        let weak = ui.downgrade();
        move |_, _| {
            if let Some(ui) = weak.upgrade() {
                ui.open_history();
            }
        }
    });
    actions.add_action(&history);
    let downloads = gio::SimpleAction::new("downloads", None);
    downloads.connect_activate({
        let weak = ui.downgrade();
        move |_, _| {
            if let Some(ui) = weak.upgrade() {
                ui.open_downloads();
            }
        }
    });
    actions.add_action(&downloads);
    actions.add_action(&logout);
    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(glib::clone!(
        #[weak]
        content,
        move |_, _| {
            // Closing the window runs the usual shutdown (playback reported
            // stopped, the SVP Manager we started stopped).
            if let Some(window) = content.root().and_downcast::<gtk::Window>() {
                super::window::shut_down(&window);
            }
        }
    ));
    actions.add_action(&quit);
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
                tracing::warn!("Could not load home: {e:#}");
                content.append(&unreachable(&ui, &content));
            }
        }
    });
}

/// Home when the server can't be reached: try again, or watch downloads.
fn unreachable(ui: &Ui, content: &gtk::Box) -> adw::StatusPage {
    let buttons = gtk::Box::builder()
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    let retry = gtk::Button::builder()
        .label("Try Again")
        .css_classes(["pill"])
        .build();
    retry.connect_clicked({
        let (weak, content) = (ui.downgrade(), content.downgrade());
        move |_| {
            if let (Some(ui), Some(content)) = (weak.upgrade(), content.upgrade()) {
                clear(&content);
                content.append(&loading());
                load(&ui, &content);
            }
        }
    });
    buttons.append(&retry);
    let downloaded = crate::downloads::list().len();
    if downloaded > 0 {
        let open = gtk::Button::builder()
            .label(format!("Downloads ({downloaded})"))
            .css_classes(["pill", "suggested-action"])
            .build();
        let weak = ui.downgrade();
        open.connect_clicked(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.open_downloads();
            }
        });
        buttons.append(&open);
    }
    adw::StatusPage::builder()
        .icon_name(crate::ui::icons::DOWNLOAD)
        .title("Can't reach your Emby server")
        .description(if downloaded > 0 {
            "Your downloads still play offline"
        } else {
            "Check that the server is on and reachable, then try again"
        })
        .child(&buttons)
        .vexpand(true)
        .build()
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

/// Reset, smaller, bigger: the UI scale, saved for the next start.
fn ui_scale_buttons(ui: &Ui) -> gtk::Box {
    let reset = gtk::Button::builder()
        .icon_name(crate::ui::icons::RESET)
        .build();
    let smaller = gtk::Button::builder()
        .icon_name("list-remove-symbolic")
        .build();
    let bigger = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .build();
    let buttons = gtk::Box::builder().spacing(2).build();
    buttons.append(
        &gtk::Label::builder()
            .label(format!("v{}", crate::update::current_version()))
            .margin_end(4)
            .css_classes(["dim-label", "caption-heading"])
            .build(),
    );
    for button in [&reset, &smaller, &bigger] {
        buttons.append(button);
    }
    let refresh = {
        let (reset, smaller, bigger) = (reset.downgrade(), smaller.downgrade(), bigger.downgrade());
        move || {
            let (Some(reset), Some(smaller), Some(bigger)) =
                (reset.upgrade(), smaller.upgrade(), bigger.upgrade())
            else {
                return;
            };
            let scale = super::ui_scale();
            let percent = (scale * 100.0).round();
            reset.set_tooltip_text(Some(&format!("Reset UI size ({percent}% now)")));
            smaller.set_tooltip_text(Some(&format!("Smaller UI ({percent}% now)")));
            bigger.set_tooltip_text(Some(&format!("Bigger UI ({percent}% now)")));
            reset.set_sensitive((scale - 1.0).abs() > 0.001);
            smaller.set_sensitive(scale > super::UI_SCALE_MIN + 0.001);
            bigger.set_sensitive(scale < super::UI_SCALE_MAX - 0.001);
        }
    };
    refresh();
    let change = {
        let weak = ui.downgrade();
        move |target: Option<f64>| {
            let scale = target.unwrap_or(1.0);
            // Whole steps, so repeated presses never drift (1.1 + 0.1 ...).
            // Divided, not multiplied, so 0.7 is stored as 0.7.
            let scale = ((scale / super::UI_SCALE_STEP).round() / super::UI_SCALE_STEP.recip())
                .clamp(super::UI_SCALE_MIN, super::UI_SCALE_MAX);
            super::apply_ui_scale(scale);
            if let Err(e) = crate::config::Settings::update(|s| s.window.ui_scale = scale) {
                tracing::warn!("could not save the UI scale: {e:#}");
            }
            // Pages rebuild with the new artwork sizes.
            if let Some(ui) = weak.upgrade() {
                ui.data_changed();
            }
            refresh();
        }
    };
    let change = std::rc::Rc::new(change);
    reset.connect_clicked({
        let change = change.clone();
        move |_| change(None)
    });
    smaller.connect_clicked({
        let change = change.clone();
        move |_| change(Some(super::ui_scale() - super::UI_SCALE_STEP))
    });
    bigger.connect_clicked(move |_| change(Some(super::ui_scale() + super::UI_SCALE_STEP)));
    buttons
}
