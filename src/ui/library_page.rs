//! A library's page. Movie and TV libraries get Emby-web-style tabs;
//! anything else (folders, collections) is one sortable grid.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::card::Shape;
use super::library::{self, PAGE_SIZE};
use super::rows::{Click, More, row};
use super::{Ui, WeakUi, clear, loading};
use crate::emby::browse::ItemQuery;
use crate::emby::models::{BaseItem, Recommendation};
use crate::runtime::spawn_tokio;

const SUGGESTION_ROW_LIMIT: usize = 20;
const RECOMMENDATION_ROWS: usize = 6;
const RECOMMENDATION_ITEMS: usize = 12;

/// Sort choices: label, Emby `SortBy`, and whether it defaults to
/// descending (newest/highest first).
const SORTS: [(&str, &str, bool); 6] = [
    ("Name", "SortName", false),
    ("Date Added", "DateCreated", true),
    ("Release Date", "PremiereDate", true),
    ("Rating", "CommunityRating", true),
    ("Runtime", "Runtime", false),
    ("Random", "Random", false),
];

/// Filter choices: label and Emby `Filters` value.
const FILTERS: [(&str, Option<&str>); 5] = [
    ("All", None),
    ("Unplayed", Some("IsUnplayed")),
    ("Played", Some("IsPlayed")),
    ("In Progress", Some("IsResumable")),
    ("Favorites", Some("IsFavorite")),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Movies,
    Shows,
}

impl Kind {
    fn of(folder: &BaseItem) -> Option<Self> {
        match folder.collection_type.as_deref() {
            Some("movies") => Some(Kind::Movies),
            Some("tvshows") => Some(Kind::Shows),
            _ => None,
        }
    }

    fn item_type(self) -> &'static str {
        match self {
            Kind::Movies => "Movie",
            Kind::Shows => "Series",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Chips {
    Genres,
    Tags,
}

pub fn page(ui: &Ui, folder: &BaseItem) -> adw::NavigationPage {
    match Kind::of(folder) {
        Some(kind) => tabbed(ui, folder, kind),
        None => plain(ui, folder),
    }
}

fn plain(ui: &Ui, folder: &BaseItem) -> adw::NavigationPage {
    let include_types = match folder.collection_type.as_deref() {
        Some("boxsets") => Some("BoxSet"),
        Some("musicvideos") => Some("MusicVideo"),
        _ => None,
    };
    let query = ItemQuery {
        parent_id: Some(folder.id.clone()),
        include_types,
        recursive: include_types.is_some(),
        ..Default::default()
    };
    toolbar_page(
        &folder.name,
        &adw::HeaderBar::new(),
        &sortable_grid(ui, query),
    )
}

/// A page whose content scrolls by itself (grids must sit directly in
/// their ScrolledWindow to stay virtualized).
fn toolbar_page(
    title: &str,
    header: &adw::HeaderBar,
    content: &impl IsA<gtk::Widget>,
) -> adw::NavigationPage {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(header);
    toolbar.set_content(Some(content));
    adw::NavigationPage::new(&toolbar, title)
}

type TabBuilder = Box<dyn FnOnce(&Ui) -> gtk::Widget>;

fn tabbed(ui: &Ui, folder: &BaseItem, kind: Kind) -> adw::NavigationPage {
    let item_type = kind.item_type();
    let library_id = folder.id.clone();
    let in_library = |include_types: Option<&'static str>, recursive: bool| ItemQuery {
        parent_id: Some(library_id.clone()),
        include_types,
        recursive,
        ..Default::default()
    };

    let mut tabs: Vec<(&str, &str, &str, TabBuilder)> = Vec::new();
    let items = in_library(Some(item_type), true);
    tabs.push((
        "items",
        if kind == Kind::Movies {
            "Movies"
        } else {
            "Shows"
        },
        if kind == Kind::Movies {
            "video-x-generic-symbolic"
        } else {
            "tv-symbolic"
        },
        Box::new(move |ui| sortable_grid(ui, items).upcast()),
    ));
    let suggestions_folder = folder.clone();
    tabs.push((
        "suggestions",
        "Suggestions",
        "view-grid-symbolic",
        Box::new(move |ui| suggestions(ui, &suggestions_folder, kind)),
    ));
    if kind == Kind::Movies {
        let collections = in_library(Some("BoxSet"), true);
        tabs.push((
            "collections",
            "Collections",
            "folder-videos-symbolic",
            Box::new(move |ui| sortable_grid(ui, collections).upcast()),
        ));
    }
    for (name, title, icon, chips_kind) in [
        ("genres", "Genres", "view-list-symbolic", Chips::Genres),
        ("tags", "Tags", "bookmark-new-symbolic", Chips::Tags),
    ] {
        let library_id = library_id.clone();
        tabs.push((
            name,
            title,
            icon,
            Box::new(move |ui| chips(ui, &library_id, item_type, chips_kind)),
        ));
    }
    let mut favorites = in_library(Some(item_type), true);
    favorites.filters.push("IsFavorite");
    tabs.push((
        "favorites",
        "Favorites",
        "starred-symbolic",
        Box::new(move |ui| sortable_grid(ui, favorites).upcast()),
    ));
    let folders = in_library(None, false);
    tabs.push((
        "folders",
        "Folders",
        "folder-symbolic",
        Box::new(move |ui| sortable_grid(ui, folders).upcast()),
    ));

    // Tabs are built the first time they're shown, so opening a library
    // only costs the requests of the tab you're looking at.
    let stack = adw::ViewStack::new();
    let pending: Rc<RefCell<HashMap<String, TabBuilder>>> = Rc::default();
    for (name, title, icon, build) in tabs {
        stack.add_titled_with_icon(&adw::Bin::new(), Some(name), title, icon);
        pending.borrow_mut().insert(name.to_string(), build);
    }
    let build_visible = {
        let weak = ui.downgrade();
        move |stack: &adw::ViewStack| {
            let (Some(ui), Some(name)) = (weak.upgrade(), stack.visible_child_name()) else {
                return;
            };
            let Some(build) = pending.borrow_mut().remove(name.as_str()) else {
                return;
            };
            if let Some(bin) = stack.child_by_name(&name).and_downcast::<adw::Bin>() {
                bin.set_child(Some(&build(&ui)));
            }
        }
    };
    build_visible(&stack);
    stack.connect_visible_child_name_notify(build_visible);

    let header = adw::HeaderBar::builder()
        .title_widget(
            &adw::ViewSwitcher::builder()
                .stack(&stack)
                .policy(adw::ViewSwitcherPolicy::Wide)
                .build(),
        )
        .build();
    toolbar_page(&folder.name, &header, &stack)
}

/// A poster grid of `base` with a count, filter, sort and order bar above.
fn sortable_grid(ui: &Ui, base: ItemQuery) -> gtk::Box {
    let (grid, store) = library::grid(ui, Shape::Poster);
    let count = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["dim-label"])
        .build();
    let filter = gtk::DropDown::from_strings(&FILTERS.map(|(label, _)| label));
    filter.set_tooltip_text(Some("Filter"));
    let sort = gtk::DropDown::from_strings(&SORTS.map(|(label, _, _)| label));
    sort.set_tooltip_text(Some("Sort by"));
    let descending = gtk::ToggleButton::builder()
        .icon_name("view-sort-ascending-symbolic")
        .tooltip_text("Sort order")
        .build();

    let bar = gtk::Box::builder()
        .spacing(6)
        .margin_start(18)
        .margin_end(18)
        .margin_top(6)
        .build();
    bar.append(&count);
    bar.append(&filter);
    bar.append(&sort);
    bar.append(&descending);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&grid)
        .vexpand(true)
        .build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.append(&bar);
    root.append(&scrolled);

    // Each reload bumps this; pages from an older query are dropped.
    let generation = Rc::new(Cell::new(0u64));
    let reload = Rc::new(glib::clone!(
        #[weak]
        store,
        #[weak]
        count,
        #[weak]
        filter,
        #[weak]
        sort,
        #[weak]
        descending,
        #[strong]
        generation,
        #[strong(rename_to = weak_ui)]
        ui.downgrade(),
        move || reload_grid(
            &weak_ui,
            &store,
            &count,
            &base,
            Choice {
                sort: SORTS[sort.selected() as usize].1,
                descending: descending.is_active(),
                filter: FILTERS[filter.selected() as usize].1,
            },
            &generation,
        )
    ));

    descending.connect_toggled({
        let reload = reload.clone();
        move |button| {
            button.set_icon_name(if button.is_active() {
                "view-sort-descending-symbolic"
            } else {
                "view-sort-ascending-symbolic"
            });
            reload();
        }
    });
    sort.connect_selected_notify(glib::clone!(
        #[weak]
        descending,
        #[strong]
        reload,
        move |sort| {
            // Each sort has a natural direction; toggling it reloads too.
            let natural = SORTS[sort.selected() as usize].2;
            if descending.is_active() != natural {
                descending.set_active(natural);
            } else {
                reload();
            }
        }
    ));
    filter.connect_selected_notify({
        let reload = reload.clone();
        move |_| reload()
    });
    reload();
    root
}

struct Choice {
    sort: &'static str,
    descending: bool,
    filter: Option<&'static str>,
}

fn reload_grid(
    ui: &WeakUi,
    store: &gtk::gio::ListStore,
    count: &gtk::Label,
    base: &ItemQuery,
    choice: Choice,
    generation: &Rc<Cell<u64>>,
) {
    let Some(ui) = ui.upgrade() else { return };
    let current = generation.get() + 1;
    generation.set(current);
    store.remove_all();
    count.set_label("");

    let mut query = base.clone();
    query.sort_by = Some(choice.sort);
    query.descending = choice.descending;
    query.filters.extend(choice.filter);
    query.limit = PAGE_SIZE;
    let still_current = generation.clone();
    let count = count.downgrade();
    library::load_pages(
        &ui,
        store,
        query,
        usize::MAX,
        move || still_current.get() == current,
        move |total| {
            if let Some(count) = count.upgrade() {
                count.set_label(&match total {
                    1 => "1 item".to_string(),
                    n => format!("{n} items"),
                });
            }
        },
    );
}

/// Genre or tag buttons; each opens a sortable grid of that genre/tag.
fn chips(ui: &Ui, library_id: &str, item_type: &'static str, kind: Chips) -> gtk::Widget {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .max_children_per_line(30)
        .row_spacing(8)
        .column_spacing(8)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .valign(gtk::Align::Start)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&loading());
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .vexpand(true)
        .build();

    let client = ui.client();
    let user_id = ui.user_id();
    let library_id = library_id.to_string();
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        let fetch_library = library_id.clone();
        let result = spawn_tokio(async move {
            match kind {
                Chips::Genres => client.genres(&user_id, &fetch_library, item_type).await,
                Chips::Tags => client.tags(&user_id, &fetch_library, item_type).await,
            }
        })
        .await;
        let Some(ui) = weak.upgrade() else { return };
        clear(&content);
        let entries = match result {
            Ok(entries) => entries,
            Err(e) => {
                ui.report_error("Could not load this tab", &e);
                return;
            }
        };
        if entries.is_empty() {
            let title = match kind {
                Chips::Genres => "No genres in this library",
                Chips::Tags => "No tags in this library",
            };
            content.append(
                &adw::StatusPage::builder()
                    .icon_name("view-list-symbolic")
                    .title(title)
                    .vexpand(true)
                    .build(),
            );
            return;
        }
        for entry in entries {
            let button = gtk::Button::builder()
                .label(&entry.name)
                .css_classes(["pill"])
                .build();
            let weak = ui.downgrade();
            let library_id = library_id.clone();
            button.connect_clicked(move |_| {
                let Some(ui) = weak.upgrade() else { return };
                let mut query = ItemQuery {
                    parent_id: Some(library_id.clone()),
                    include_types: Some(item_type),
                    recursive: true,
                    ..Default::default()
                };
                match kind {
                    Chips::Genres => query.genre_id = Some(entry.id.clone()),
                    Chips::Tags => query.tag_id = Some(entry.id.clone()),
                }
                let grid = sortable_grid(&ui, query);
                ui.push(&toolbar_page(&entry.name, &adw::HeaderBar::new(), &grid));
            });
            flow.append(&button);
        }
        content.append(&flow);
    });
    scrolled.upcast()
}

struct Suggestions {
    resume: Vec<BaseItem>,
    next_up: Vec<BaseItem>,
    latest: Vec<BaseItem>,
    recommendations: Vec<Recommendation>,
}

/// The library's own home: what's in progress, what's new, and what's
/// similar to what you watched.
fn suggestions(ui: &Ui, folder: &BaseItem, kind: Kind) -> gtk::Widget {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(24)
        .margin_top(12)
        .margin_bottom(24)
        .build();
    content.append(&loading());
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&content)
        .vexpand(true)
        .build();

    let client = ui.client();
    let user_id = ui.user_id();
    let library_id = folder.id.clone();
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        let fetch_library = library_id.clone();
        let result = spawn_tokio(async move {
            let parent = Some(fetch_library.as_str());
            let (resume, next_up, latest, recommendations) = tokio::join!(
                client.resume(&user_id, parent, SUGGESTION_ROW_LIMIT),
                async {
                    match kind {
                        Kind::Shows => client.next_up(&user_id, parent, SUGGESTION_ROW_LIMIT).await,
                        Kind::Movies => Ok(Vec::new()),
                    }
                },
                client.latest(&user_id, &fetch_library, SUGGESTION_ROW_LIMIT),
                async {
                    match kind {
                        Kind::Movies => {
                            client
                                .movie_recommendations(
                                    &user_id,
                                    &fetch_library,
                                    RECOMMENDATION_ROWS,
                                    RECOMMENDATION_ITEMS,
                                )
                                .await
                        }
                        Kind::Shows => Ok(Vec::new()),
                    }
                },
            );
            // Rows are independent; one failing shouldn't hide the others.
            let or_empty = |name: &str, result: anyhow::Result<Vec<_>>| {
                result.unwrap_or_else(|e| {
                    tracing::warn!("{name} row failed: {e:#}");
                    Vec::new()
                })
            };
            Suggestions {
                resume: or_empty("resume", resume),
                next_up: or_empty("next up", next_up),
                latest: or_empty("latest", latest),
                recommendations: recommendations.unwrap_or_else(|e| {
                    tracing::warn!("recommendations failed: {e:#}");
                    Vec::new()
                }),
            }
        })
        .await;
        let Some(ui) = weak.upgrade() else { return };
        clear(&content);

        let mut any = false;
        let mut add = |widget: gtk::Box| {
            content.append(&widget);
            any = true;
        };
        if !result.resume.is_empty() {
            add(row(
                &ui,
                "Continue Watching",
                &result.resume,
                Shape::Landscape,
                Click::Resume,
                More::Resume(Some(library_id.clone())),
            ));
        }
        if !result.next_up.is_empty() {
            add(row(
                &ui,
                "Next Up",
                &result.next_up,
                Shape::Landscape,
                Click::Open,
                More::NextUp(Some(library_id.clone())),
            ));
        }
        if !result.latest.is_empty() {
            add(row(
                &ui,
                "Latest",
                &result.latest,
                Shape::Poster,
                Click::Open,
                More::None,
            ));
        }
        for recommendation in result.recommendations {
            if recommendation.items.is_empty() {
                continue;
            }
            add(row(
                &ui,
                &recommendation.title(),
                &recommendation.items,
                Shape::Poster,
                Click::Open,
                More::None,
            ));
        }
        if !any {
            content.append(
                &adw::StatusPage::builder()
                    .icon_name("view-grid-symbolic")
                    .title("Nothing to suggest yet")
                    .description("Watch something in this library first")
                    .vexpand(true)
                    .build(),
            );
        }
    });
    scrolled.upcast()
}
