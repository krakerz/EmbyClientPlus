//! Search across all libraries.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;

use super::card::Shape;
use super::library::{grid, load_pages};
use super::{Ui, scrolled_page};
use crate::emby::browse::ItemQuery;

const RESULT_LIMIT: usize = 60;

pub fn page(ui: &Ui, term: &str) -> adw::NavigationPage {
    let (grid, store) = grid(ui, Shape::Poster);
    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Movies, shows, episodes")
        .search_delay(300)
        .hexpand(true)
        .build();
    let header = adw::HeaderBar::builder()
        .title_widget(
            &adw::Clamp::builder()
                .maximum_size(480)
                .child(&entry)
                .build(),
        )
        .build();

    let empty = adw::StatusPage::builder()
        .icon_name(crate::ui::icons::SEARCH)
        .title("Search your library")
        .vexpand(true)
        .build();
    let stack = gtk::Stack::new();
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&grid, Some("results"));
    let page = scrolled_page("Search", None, &header, &stack);

    // The cursor lands on the first result, ready for the controller,
    // unless the user is typing a new search in the entry.
    store.connect_items_changed({
        let (grid, entry) = (grid.clone(), entry.clone());
        move |store, _, _, _| {
            let typing = grid
                .root()
                .and_then(|root| root.focus())
                .is_some_and(|focus| focus.is_ancestor(&entry));
            if store.n_items() > 0 && !typing {
                grid.scroll_to(0, gtk::ListScrollFlags::FOCUS, None);
            }
        }
    });
    // Each query bumps this; pages of an older query are discarded.
    let generation = Rc::new(Cell::new(0u64));
    let weak = ui.downgrade();
    entry.connect_search_changed(move |entry| {
        let Some(ui) = weak.upgrade() else { return };
        let current = generation.get() + 1;
        generation.set(current);
        store.remove_all();
        let term = entry.text().trim().to_string();
        if term.is_empty() {
            stack.set_visible_child_name("empty");
            return;
        }
        stack.set_visible_child_name("results");
        let query = ItemQuery {
            search_term: Some(term),
            include_types: Some("Movie,Series,Episode"),
            recursive: true,
            limit: RESULT_LIMIT,
            ..Default::default()
        };
        let generation = generation.clone();
        // One page is plenty for search.
        load_pages(
            &ui,
            &store,
            query,
            RESULT_LIMIT,
            move || generation.get() == current,
            |_| {},
        );
    });
    // Fires search-changed, which runs the query.
    entry.set_text(term);
    page
}
