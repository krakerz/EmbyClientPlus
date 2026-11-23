//! Search across all libraries, with results in tabs by kind (Top
//! Results, Movies, Shows, Episodes, People, Genres, ...). Tabs without matches
//! stay hidden.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use super::card::Shape;
use super::library::{grid, load_pages};
use super::{Ui, add_home_button, icons, search_names};
use crate::emby::browse::ItemQuery;
use crate::runtime::spawn_tokio;

const RESULT_LIMIT: usize = 60;

/// Where a tab's results come from.
#[derive(Clone, Copy)]
enum Source {
    Items(&'static str),
    People,
    Artists,
    Genres,
    Tags,
}

/// (name, title, icon, card shape, source), in tab order.
const TABS: [(&str, &str, &str, Shape, Source); 11] = [
    (
        "top",
        "Top Results",
        icons::SEARCH,
        Shape::Poster,
        Source::Items("Movie,Series,BoxSet,MusicAlbum"),
    ),
    (
        "movies",
        "Movies",
        icons::MOVIES,
        Shape::Poster,
        Source::Items("Movie"),
    ),
    (
        "shows",
        "Shows",
        icons::SHOWS,
        Shape::Poster,
        Source::Items("Series"),
    ),
    (
        "episodes",
        "Episodes",
        icons::EPISODE,
        Shape::Landscape,
        Source::Items("Episode"),
    ),
    (
        "people",
        "People",
        icons::PERSON,
        Shape::Person,
        Source::People,
    ),
    (
        "collections",
        "Collections",
        icons::COLLECTIONS,
        Shape::Poster,
        Source::Items("BoxSet"),
    ),
    (
        "genres",
        "Genres",
        icons::GENRES,
        Shape::Landscape,
        Source::Genres,
    ),
    ("tags", "Tags", icons::TAGS, Shape::Landscape, Source::Tags),
    (
        "albums",
        "Albums",
        icons::ALBUMS,
        Shape::Square,
        Source::Items("MusicAlbum"),
    ),
    (
        "songs",
        "Songs",
        icons::SONGS,
        Shape::Square,
        Source::Items("Audio"),
    ),
    (
        "artists",
        "Artists",
        icons::ARTISTS,
        Shape::Person,
        Source::Artists,
    ),
];

struct Tab {
    page: adw::ViewStackPage,
    grid: gtk::GridView,
    store: gio::ListStore,
    /// Genres and tags show as category tiles instead of the grid.
    tiles: Option<gtk::FlowBox>,
    source: Source,
}

pub fn page(ui: &Ui, term: &str) -> adw::NavigationPage {
    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Movies, shows, episodes, people, genres, tags, music")
        .search_delay(300)
        .hexpand(true)
        .build();
    let header = adw::HeaderBar::builder()
        .title_widget(
            &adw::Clamp::builder()
                .maximum_size(560)
                .child(&entry)
                .build(),
        )
        .build();
    add_home_button(&header);

    let results = adw::ViewStack::new();
    let tabs: Rc<Vec<Tab>> = Rc::new(
        TABS.iter()
            .map(|&(name, title, icon, shape, source)| {
                let (grid, store) = grid(ui, shape);
                let tiles = matches!(source, Source::Genres | Source::Tags)
                    .then(|| super::category_tile::flow(ui));
                let scrolled = gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .vexpand(true)
                    .build();
                match &tiles {
                    Some(tiles) => {
                        // Same layout as the library's Genres/Tags tabs.
                        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
                        content.append(tiles);
                        scrolled.set_child(Some(&content));
                    }
                    None => scrolled.set_child(Some(&grid)),
                }
                let page = results.add_titled_with_icon(&scrolled, Some(name), title, icon);
                page.set_visible(false);
                Tab {
                    page,
                    grid,
                    store,
                    tiles,
                    source,
                }
            })
            .collect(),
    );
    let switcher = adw::ViewSwitcher::builder()
        .stack(&results)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .halign(gtk::Align::Center)
        .margin_bottom(6)
        .build();

    let empty = adw::StatusPage::builder()
        .icon_name(icons::SEARCH)
        .title("Search your library")
        .vexpand(true)
        .build();
    let nothing = adw::StatusPage::builder()
        .icon_name(icons::SEARCH)
        .title("No results")
        .vexpand(true)
        .build();
    let stack = gtk::Stack::new();
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&nothing, Some("nothing"));
    stack.add_named(&results, Some("results"));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_top_bar(&switcher);
    toolbar.set_content(Some(&stack));
    let page = adw::NavigationPage::new(&toolbar, "Search");
    {
        let results = results.clone();
        super::set_tab_stepper(&page, move |forward| {
            super::library_page::step_stack(&results, forward)
        });
    }

    // Each query bumps this; results of an older query are discarded.
    let generation = Rc::new(Cell::new(0u64));
    let weak = ui.downgrade();
    {
        let (tabs, results, stack, switcher, entry_ref) = (
            tabs.clone(),
            results.clone(),
            stack.clone(),
            switcher.clone(),
            entry.clone(),
        );
        entry.connect_search_changed(move |entry| {
            let Some(ui) = weak.upgrade() else { return };
            let current = generation.get() + 1;
            generation.set(current);
            let term = entry.text().trim().to_string();
            for tab in tabs.iter() {
                tab.store.remove_all();
                if let Some(tiles) = &tab.tiles {
                    while let Some(child) = tiles.first_child() {
                        tiles.remove(&child);
                    }
                }
                tab.page.set_visible(false);
            }
            switcher.set_visible(false);
            if term.is_empty() {
                stack.set_visible_child_name("empty");
                return;
            }
            stack.set_visible_child_name("nothing");
            // Results come in per tab. Once every tab before it has
            // answered, the first one with results is shown (once).
            let answers: Rc<RefCell<Vec<Option<bool>>>> =
                Rc::new(RefCell::new(vec![None; tabs.len()]));
            let chosen = Rc::new(Cell::new(false));
            for index in 0..tabs.len() {
                let found = {
                    let (tabs, results, stack, switcher, entry) = (
                        tabs.clone(),
                        results.clone(),
                        stack.clone(),
                        switcher.clone(),
                        entry_ref.clone(),
                    );
                    let (generation, answers, chosen) =
                        (generation.clone(), answers.clone(), chosen.clone());
                    move |total: usize| {
                        if generation.get() != current {
                            return;
                        }
                        answers.borrow_mut()[index] = Some(total > 0);
                        if total > 0 {
                            tabs[index].page.set_visible(true);
                            switcher.set_visible(true);
                            stack.set_visible_child_name("results");
                        }
                        let first = answers.borrow().iter().position(|a| *a != Some(false));
                        if let Some(first) = first
                            && answers.borrow()[first] == Some(true)
                            && !chosen.replace(true)
                        {
                            show_tab(&tabs[first], &results, &entry);
                        }
                    }
                };
                let still_wanted = {
                    let generation = generation.clone();
                    move || generation.get() == current
                };
                load_tab(&ui, &tabs[index], &term, still_wanted, found);
            }
        });
    }
    // Fires search-changed, which runs the query.
    entry.set_text(term);
    page
}

/// Shows `tab`, cursor on its first card (unless the user is typing a
/// new search).
fn show_tab(tab: &Tab, results: &adw::ViewStack, entry: &gtk::SearchEntry) {
    results.set_visible_child(&tab.page.child());
    let typing = entry
        .root()
        .and_then(|root| root.focus())
        .is_some_and(|focus| focus.is_ancestor(entry));
    if typing {
        return;
    }
    if let Some(tiles) = &tab.tiles {
        // Tiles are in before the count is reported.
        if let Some(first) = tiles.child_at_index(0) {
            first.grab_focus();
        }
        return;
    }
    if tab.store.n_items() > 0 {
        tab.grid.scroll_to(0, gtk::ListScrollFlags::FOCUS, None);
        return;
    }
    // The count comes just before the cards; focus once they're in.
    let grid = tab.grid.clone();
    let handler = Rc::new(Cell::new(None::<glib::SignalHandlerId>));
    let id = tab.store.connect_items_changed({
        let handler = handler.clone();
        move |store, _, _, _| {
            if store.n_items() > 0 {
                grid.scroll_to(0, gtk::ListScrollFlags::FOCUS, None);
                if let Some(id) = handler.take() {
                    store.disconnect(id);
                }
            }
        }
    });
    handler.set(Some(id));
}

/// Emby's results first, then items whose title merely contains the text
/// (Emby's `SearchTerm` matches word starts only).
fn load_with_name_matches(
    ui: &Ui,
    tab: &Tab,
    query: ItemQuery,
    name_types: Vec<&'static str>,
    still_wanted: impl Fn() -> bool + 'static,
    found: impl Fn(usize) + 'static,
) {
    let (client, user_id) = (ui.client(), ui.user_id());
    let term = query.search_term.clone().unwrap_or_default();
    let store = tab.store.downgrade();
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let page = spawn_tokio({
            let (client, user_id, query) = (client.clone(), user_id.clone(), query.clone());
            async move { client.items(&user_id, &query).await }
        })
        .await;
        if !still_wanted() {
            return;
        }
        let page = match page {
            Ok(page) => page,
            Err(e) => {
                ui.report_error("Could not load items", &e);
                return;
            }
        };
        let mut seen: HashSet<String> = page.items.iter().map(|i| i.id.clone()).collect();
        let mut extra_ids = Vec::new();
        for item_type in name_types {
            let room = RESULT_LIMIT.saturating_sub(page.items.len() + extra_ids.len());
            let ids = search_names::find(&ui, item_type, &term, &seen, room).await;
            if !still_wanted() {
                return;
            }
            seen.extend(ids.iter().cloned());
            extra_ids.extend(ids);
        }
        let mut extras = Vec::new();
        if !extra_ids.is_empty() {
            let extra_query = ItemQuery {
                ids: Some(extra_ids.join(",")),
                include_types: query.include_types,
                recursive: true,
                sort_by: Some("SortName"),
                limit: extra_ids.len(),
                ..Default::default()
            };
            match spawn_tokio(async move { client.items(&user_id, &extra_query).await }).await {
                Ok(r) => extras = r.items,
                Err(e) => tracing::warn!("search extra items failed: {e:#}"),
            }
            if !still_wanted() {
                return;
            }
        }
        let Some(store) = store.upgrade() else {
            return;
        };
        found(page.total_record_count.max(page.items.len()) + extras.len());
        let objects: Vec<_> = page
            .items
            .into_iter()
            .chain(extras)
            .map(glib::BoxedAnyObject::new)
            .collect();
        store.extend_from_slice(&objects);
    });
}

fn load_tab(
    ui: &Ui,
    tab: &Tab,
    term: &str,
    still_wanted: impl Fn() -> bool + 'static,
    found: impl Fn(usize) + 'static,
) {
    match tab.source {
        Source::Items(types) => {
            let query = ItemQuery {
                search_term: Some(term.to_string()),
                include_types: Some(types),
                recursive: true,
                limit: RESULT_LIMIT,
                ..Default::default()
            };
            let name_types = search_names::matching_types(types);
            if name_types.is_empty() {
                // One page is plenty for search.
                load_pages(ui, &tab.store, query, RESULT_LIMIT, still_wanted, found);
            } else {
                load_with_name_matches(ui, tab, query, name_types, still_wanted, found);
            }
        }
        source => {
            let (client, user_id, term) = (ui.client(), ui.user_id(), term.to_string());
            let store = tab.store.downgrade();
            let tiles = tab.tiles.as_ref().map(|t| t.downgrade());
            let weak = ui.downgrade();
            glib::spawn_future_local(async move {
                let result = spawn_tokio(async move {
                    match source {
                        Source::People => client.search_people(&user_id, &term, RESULT_LIMIT).await,
                        Source::Genres => {
                            client
                                .search_categories("Genres", &user_id, &term, RESULT_LIMIT)
                                .await
                        }
                        Source::Tags => {
                            client
                                .search_categories("Tags", &user_id, &term, RESULT_LIMIT)
                                .await
                        }
                        _ => client.search_artists(&user_id, &term, RESULT_LIMIT).await,
                    }
                })
                .await;
                if !still_wanted() {
                    return;
                }
                let mut items = result.map(|r| r.items).unwrap_or_else(|e| {
                    tracing::warn!("search failed: {e:#}");
                    Vec::new()
                });
                // Emby doesn't type tags as "Tag", so opening one would
                // treat it as a folder; the endpoint says what they are.
                let category = match source {
                    Source::Genres => Some("Genre"),
                    Source::Tags => Some("Tag"),
                    _ => None,
                };
                if let Some(category) = category {
                    for item in &mut items {
                        item.item_type = category.into();
                    }
                }
                if let (Some(tiles), Some(ui)) = (tiles.and_then(|t| t.upgrade()), weak.upgrade()) {
                    for item in &items {
                        let query = super::library_page::category_query(item);
                        tiles.append(&super::category_tile::tile(&ui, &item.name, query));
                    }
                    super::category_tile::fit(&tiles, items.len());
                    found(items.len());
                    return;
                }
                found(items.len());
                if let Some(store) = store.upgrade() {
                    let objects: Vec<_> =
                        items.into_iter().map(glib::BoxedAnyObject::new).collect();
                    store.extend_from_slice(&objects);
                }
            });
        }
    }
}
