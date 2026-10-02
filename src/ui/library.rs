//! Card grids over a `gio::ListStore`, filled from paged `/Items`
//! queries or one-off fetches. Library pages, search and full-row pages
//! build on these.

use std::future::Future;
use std::sync::Arc;

use adw::prelude::*;
use anyhow::Result;
use gtk::{gio, glib};

use super::card::{Card, Shape};
use super::{Ui, scrolled_page};
use crate::emby::EmbyClient;
use crate::emby::browse::ItemQuery;
use crate::emby::models::BaseItem;
use crate::runtime::spawn_tokio;

pub const PAGE_SIZE: usize = 100;

/// A grid of one-off results that aren't a `/Items` listing (Continue
/// Watching, Next Up), fetched once.
pub fn list_page<F, Fut>(ui: &Ui, title: &str, shape: Shape, fetch: F) -> adw::NavigationPage
where
    F: FnOnce(Arc<EmbyClient>, String) -> Fut,
    Fut: Future<Output = Result<Vec<BaseItem>>> + Send + 'static,
{
    let (grid, store) = grid(ui, shape);
    let header = adw::HeaderBar::new();
    let page = scrolled_page(title, None, &header, &grid);
    let fetching = fetch(ui.client(), ui.user_id());
    let ui = ui.clone();
    let context = format!("Could not load {title}");
    glib::spawn_future_local(async move {
        match spawn_tokio(fetching).await {
            Ok(items) => {
                let objects: Vec<_> = items.into_iter().map(glib::BoxedAnyObject::new).collect();
                store.extend_from_slice(&objects);
            }
            Err(e) => ui.report_error(&context, &e),
        }
    });
    page
}

/// A grid of cards over a list store of `BaseItem`s; activating a card
/// opens it.
pub fn grid(ui: &Ui, shape: Shape) -> (gtk::GridView, gio::ListStore) {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, list_item| {
        let card = Card::new(shape);
        list_item
            .downcast_ref::<gtk::ListItem>()
            .expect("grid factory only creates ListItems")
            .set_child(Some(&card.root));
    });
    let weak = ui.downgrade();
    factory.connect_bind(move |_, list_item| {
        let Some(ui) = weak.upgrade() else { return };
        let list_item = list_item
            .downcast_ref::<gtk::ListItem>()
            .expect("grid factory only creates ListItems");
        let (Some(root), Some(object)) = (
            list_item.child().and_downcast::<gtk::Box>(),
            list_item.item().and_downcast::<glib::BoxedAnyObject>(),
        ) else {
            return;
        };
        Card::from_root(&root).bind(&ui, &object.borrow::<BaseItem>(), shape);
    });

    let grid = gtk::GridView::builder()
        .model(&gtk::NoSelection::new(Some(store.clone())))
        .factory(&factory)
        .min_columns(2)
        .max_columns(12)
        .single_click_activate(true)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .css_classes(["poster-grid"])
        .build();
    let weak = ui.downgrade();
    grid.connect_activate(move |grid, position| {
        let (Some(ui), Some(object)) = (
            weak.upgrade(),
            grid.model()
                .and_then(|model| model.item(position))
                .and_downcast::<glib::BoxedAnyObject>(),
        ) else {
            return;
        };
        let item = object.borrow::<BaseItem>().clone();
        ui.open(&item);
    });
    (grid, store)
}

/// Appends pages of `query` to `store` until `max_items` are in, stopping
/// early if the grid went away (page popped) or `still_wanted` says the
/// results are stale.
pub fn load_pages(
    ui: &Ui,
    store: &gio::ListStore,
    mut query: ItemQuery,
    max_items: usize,
    still_wanted: impl Fn() -> bool + 'static,
    on_total: impl Fn(usize) + 'static,
) {
    let client = ui.client();
    let user_id = ui.user_id();
    let weak_ui = ui.downgrade();
    let weak_store = store.downgrade();
    glib::spawn_future_local(async move {
        loop {
            let page = spawn_tokio({
                let client = client.clone();
                let user_id = user_id.clone();
                let query = query.clone();
                async move { client.items(&user_id, &query).await }
            })
            .await;
            let Some(store) = weak_store.upgrade() else {
                return;
            };
            if !still_wanted() {
                return;
            }
            let page = match page {
                Ok(page) => page,
                Err(e) => {
                    if let Some(ui) = weak_ui.upgrade() {
                        ui.report_error("Could not load items", &e);
                    }
                    return;
                }
            };
            if query.start == 0 {
                on_total(page.total_record_count);
            }
            let received = page.items.len();
            let objects: Vec<_> = page
                .items
                .into_iter()
                .map(glib::BoxedAnyObject::new)
                .collect();
            store.extend_from_slice(&objects);
            query.start += received;
            if received == 0 || query.start >= page.total_record_count || query.start >= max_items {
                return;
            }
        }
    });
}
