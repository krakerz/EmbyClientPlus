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
    if folder.collection_type.as_deref() == Some("music") {
        return music(ui, folder);
    }
    match Kind::of(folder) {
        Some(kind) => tabbed(ui, folder, kind),
        None => plain(ui, folder),
    }
}

/// Everything a person appears in.
pub fn person(ui: &Ui, person: &BaseItem) -> adw::NavigationPage {
    let query = ItemQuery {
        person_id: Some(person.id.clone()),
        include_types: Some("Movie,Series"),
        recursive: true,
        ..Default::default()
    };
    toolbar_page(
        &person.name,
        &adw::HeaderBar::new(),
        &sortable_grid(ui, query),
    )
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

/// A titled page holding a sortable grid of `query`.
pub fn grid_page(ui: &Ui, title: &str, query: ItemQuery) -> adw::NavigationPage {
    toolbar_page(title, &adw::HeaderBar::new(), &sortable_grid(ui, query))
}

/// A page whose content scrolls by itself (grids must sit directly in
/// their ScrolledWindow to stay virtualized).
fn toolbar_page(
    title: &str,
    header: &adw::HeaderBar,
    content: &impl IsA<gtk::Widget>,
) -> adw::NavigationPage {
    super::add_home_button(header);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(header);
    toolbar.set_content(Some(content));
    adw::NavigationPage::new(&toolbar, title)
}

type TabBuilder = Box<dyn FnOnce(&Ui) -> gtk::Widget>;
/// Name, title, icon, and how to build it.
type Tab = (&'static str, &'static str, &'static str, TabBuilder);

fn tabbed(ui: &Ui, folder: &BaseItem, kind: Kind) -> adw::NavigationPage {
    let item_type = kind.item_type();
    let library_id = folder.id.clone();
    let in_library = |include_types: Option<&'static str>, recursive: bool| ItemQuery {
        parent_id: Some(library_id.clone()),
        include_types,
        recursive,
        ..Default::default()
    };

    let mut tabs: Vec<Tab> = Vec::new();
    let items = in_library(Some(item_type), true);
    tabs.push((
        "items",
        if kind == Kind::Movies {
            "Movies"
        } else {
            "Shows"
        },
        if kind == Kind::Movies {
            crate::ui::icons::MOVIES
        } else {
            crate::ui::icons::SHOWS
        },
        Box::new(move |ui| sortable_grid(ui, items).upcast()),
    ));
    let suggestions_folder = folder.clone();
    tabs.push((
        "suggestions",
        "Suggestions",
        crate::ui::icons::SUGGESTIONS,
        Box::new(move |ui| suggestions(ui, &suggestions_folder, kind)),
    ));
    if kind == Kind::Movies {
        let collections = in_library(Some("BoxSet"), true);
        tabs.push((
            "collections",
            "Collections",
            crate::ui::icons::COLLECTIONS,
            Box::new(move |ui| sortable_grid(ui, collections).upcast()),
        ));
    }
    for (name, title, icon, chips_kind) in [
        ("genres", "Genres", crate::ui::icons::GENRES, Chips::Genres),
        ("tags", "Tags", crate::ui::icons::TAGS, Chips::Tags),
    ] {
        let library_id = library_id.clone();
        tabs.push((
            name,
            title,
            icon,
            Box::new(move |ui| chips(ui, &library_id, item_type, chips_kind)),
        ));
    }
    let favorites_library = folder.id.clone();
    let collection = folder.collection_type.clone();
    tabs.push((
        "favorites",
        "Favorites",
        crate::ui::icons::FAVORITE,
        Box::new(move |ui| {
            super::favorites::view(
                ui,
                Some(favorites_library),
                super::favorites::sections_for(collection.as_deref()),
            )
        }),
    ));
    let folders = in_library(None, false);
    tabs.push((
        "folders",
        "Folders",
        crate::ui::icons::FOLDER,
        Box::new(move |ui| sortable_grid(ui, folders).upcast()),
    ));

    tab_page(ui, folder, tabs)
}

/// A library page with these tabs in its header.
fn tab_page(ui: &Ui, folder: &BaseItem, tabs: Vec<Tab>) -> adw::NavigationPage {
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
    let page = toolbar_page(&folder.name, &header, &stack);
    super::set_tab_stepper(&page, move |forward| step_stack(&stack, forward));
    page
}

/// A music library: albums, artists, songs, what's new, favourites, folders.
fn music(ui: &Ui, folder: &BaseItem) -> adw::NavigationPage {
    let library_id = folder.id.clone();
    let in_library = |include_types: &'static str| ItemQuery {
        parent_id: Some(library_id.clone()),
        include_types: Some(include_types),
        recursive: true,
        ..Default::default()
    };
    let albums = in_library("MusicAlbum");
    let songs = in_library("Audio");
    let folders = ItemQuery {
        parent_id: Some(library_id.clone()),
        ..Default::default()
    };
    let artists_library = library_id.clone();
    let latest_library = library_id.clone();
    let favorites_library = library_id.clone();
    let tabs: Vec<Tab> = vec![
        (
            "albums",
            "Albums",
            crate::ui::icons::ALBUMS,
            Box::new(move |ui| sortable_grid_of(ui, albums, Shape::Square).upcast()),
        ),
        (
            "artists",
            "Artists",
            crate::ui::icons::ARTISTS,
            Box::new(move |ui| artists_grid(ui, &artists_library)),
        ),
        (
            "songs",
            "Songs",
            crate::ui::icons::SONGS,
            Box::new(move |ui| sortable_grid_of(ui, songs, Shape::Square).upcast()),
        ),
        (
            "latest",
            "Latest",
            crate::ui::icons::SUGGESTIONS,
            Box::new(move |ui| latest_albums(ui, &latest_library)),
        ),
        (
            "favorites",
            "Favorites",
            crate::ui::icons::FAVORITE,
            Box::new(move |ui| {
                super::favorites::view(
                    ui,
                    Some(favorites_library),
                    super::favorites::sections_for(Some("music")),
                )
            }),
        ),
        (
            "folders",
            "Folders",
            crate::ui::icons::FOLDER,
            Box::new(move |ui| sortable_grid_of(ui, folders, Shape::Square).upcast()),
        ),
    ];
    tab_page(ui, folder, tabs)
}

/// Album artists as round cards; each opens their albums.
fn artists_grid(ui: &Ui, library_id: &str) -> gtk::Widget {
    let (grid, store) = library::grid(ui, Shape::Person);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&grid)
        .vexpand(true)
        .build();
    let client = ui.client();
    let user_id = ui.user_id();
    let library_id = library_id.to_string();
    let weak = ui.downgrade();
    glib::spawn_future_local(async move {
        let result =
            spawn_tokio(async move { client.album_artists(&user_id, &library_id).await }).await;
        match result {
            Ok(artists) => {
                let objects: Vec<_> = artists.into_iter().map(glib::BoxedAnyObject::new).collect();
                store.extend_from_slice(&objects);
            }
            Err(e) => {
                if let Some(ui) = weak.upgrade() {
                    ui.report_error("Could not load artists", &e);
                }
            }
        }
    });
    scrolled.upcast()
}

/// Recently added albums.
fn latest_albums(ui: &Ui, library_id: &str) -> gtk::Widget {
    let library_id = library_id.to_string();
    let (grid, store) = library::grid(ui, Shape::Square);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&grid)
        .vexpand(true)
        .build();
    let client = ui.client();
    let user_id = ui.user_id();
    glib::spawn_future_local(async move {
        if let Ok(albums) =
            spawn_tokio(async move { client.latest(&user_id, &library_id, 60).await }).await
        {
            let objects: Vec<_> = albums.into_iter().map(glib::BoxedAnyObject::new).collect();
            store.extend_from_slice(&objects);
        }
    });
    scrolled.upcast()
}

/// An artist's albums.
pub fn artist(ui: &Ui, artist: &BaseItem) -> adw::NavigationPage {
    let query = ItemQuery {
        artist_id: Some(artist.id.clone()),
        include_types: Some("MusicAlbum"),
        recursive: true,
        sort_by: Some("ProductionYear"),
        descending: true,
        ..Default::default()
    };
    toolbar_page(
        &artist.name,
        &adw::HeaderBar::new(),
        &sortable_grid_of(ui, query, Shape::Square),
    )
}

/// Shows the next/previous tab of `stack` (controller bumpers).
fn step_stack(stack: &adw::ViewStack, forward: bool) {
    let pages = stack.pages();
    let count = pages.n_items();
    let current = (0..count).find(|&i| {
        pages
            .item(i)
            .and_downcast::<adw::ViewStackPage>()
            .is_some_and(|page| Some(page.child()) == stack.visible_child())
    });
    let Some(current) = current else { return };
    let next = if forward {
        (current + 1).min(count.saturating_sub(1))
    } else {
        current.saturating_sub(1)
    };
    if let Some(page) = pages.item(next).and_downcast::<adw::ViewStackPage>() {
        stack.set_visible_child(&page.child());
    }
}

/// A poster grid of `base` with a count, filter, sort and order bar above.
fn sortable_grid(ui: &Ui, base: ItemQuery) -> gtk::Box {
    sortable_grid_of(ui, base, Shape::Poster)
}

fn sortable_grid_of(ui: &Ui, base: ItemQuery, shape: Shape) -> gtk::Box {
    let (grid, store) = library::grid(ui, shape);
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
        .icon_name(crate::ui::icons::SORT_ASCENDING)
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
                crate::ui::icons::SORT_DESCENDING
            } else {
                crate::ui::icons::SORT_ASCENDING
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

/// Genre or tag tiles; each opens a sortable grid of that genre/tag.
fn chips(ui: &Ui, library_id: &str, item_type: &'static str, kind: Chips) -> gtk::Widget {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .activate_on_single_click(true)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(8)
        .row_spacing(16)
        .column_spacing(16)
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
    let tiles = flow.clone();
    glib::spawn_future_local(async move {
        let flow = tiles;
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
                    .icon_name(crate::ui::icons::GENRES)
                    .title(title)
                    .vexpand(true)
                    .build(),
            );
            return;
        }
        for entry in entries {
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
            flow.append(&super::category_tile::tile(&ui, &entry.name, query));
        }
        content.append(&flow);
    });
    flow.connect_child_activated({
        let weak = ui.downgrade();
        move |_, child| {
            let Some(ui) = weak.upgrade() else { return };
            if let Some((name, query)) = super::category_tile::target(child) {
                let grid = sortable_grid(&ui, query);
                ui.push(&toolbar_page(&name, &adw::HeaderBar::new(), &grid));
            }
        }
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
                Shape::for_episodes(),
                Click::Resume,
                More::Resume(Some(library_id.clone())),
            ));
        }
        if !result.next_up.is_empty() {
            add(row(
                &ui,
                "Next Up",
                &result.next_up,
                Shape::for_episodes(),
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
                    .icon_name(crate::ui::icons::SUGGESTIONS)
                    .title("Nothing to suggest yet")
                    .description("Watch something in this library first")
                    .vexpand(true)
                    .build(),
            );
        }
    });
    scrolled.upcast()
}
