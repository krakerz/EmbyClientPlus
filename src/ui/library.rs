//! Card grids over a `gio::ListStore`, filled from paged `/Items`
//! queries or one-off fetches. Library pages, search and full-row pages
//! build on these.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use anyhow::Result;
use gtk::{gio, glib};

use super::card::{Card, Shape, attach_menu};
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
    let weak = ui.downgrade();
    factory.connect_setup(move |_, list_item| {
        let card = Card::new(shape);
        let list_item = list_item
            .downcast_ref::<gtk::ListItem>()
            .expect("grid factory only creates ListItems");
        list_item.set_child(Some(&card.root));
        if let Some(ui) = weak.upgrade() {
            let cell = list_item.downgrade();
            attach_menu(&ui, &card.root, move || {
                let object = cell
                    .upgrade()?
                    .item()
                    .and_downcast::<glib::BoxedAnyObject>()?;
                Some(object.borrow::<BaseItem>().clone())
            });
        }
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
        set_grid_position(&root, list_item.position());
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
    fit_columns(&grid, shape);
    LIVE_GRIDS.with(|grids| {
        grids.borrow_mut().push(LiveGrid {
            grid: grid.downgrade(),
            store: store.downgrade(),
            shape,
        });
    });
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

/// A grid made by `grid()`, kept so a changed item can be redrawn in place.
struct LiveGrid {
    grid: glib::WeakRef<gtk::GridView>,
    store: glib::WeakRef<gio::ListStore>,
    shape: Shape,
}

thread_local! {
    static LIVE_GRIDS: RefCell<Vec<LiveGrid>> = const { RefCell::new(Vec::new()) };
}

/// Shows `item`'s new state (watched, favourite, unplayed count) on its
/// card in every live grid. The store is edited in place, not spliced, so
/// the controller focus and scroll position stay put.
pub fn refresh_item(ui: &Ui, item: &BaseItem) {
    // Collected first: binding must not run under the registry borrow.
    let live: Vec<_> = LIVE_GRIDS.with(|grids| {
        let mut grids = grids.borrow_mut();
        grids.retain(|g| g.grid.upgrade().is_some() && g.store.upgrade().is_some());
        grids
            .iter()
            .filter_map(|g| Some((g.grid.upgrade()?, g.store.upgrade()?, g.shape)))
            .collect()
    });
    for (grid, store, shape) in live {
        let positions: Vec<u32> = (0..store.n_items())
            .filter(|&position| {
                store
                    .item(position)
                    .and_downcast::<glib::BoxedAnyObject>()
                    .is_some_and(|object| {
                        let matches = object.borrow::<BaseItem>().id == item.id;
                        if matches {
                            *object.borrow_mut::<BaseItem>() = item.clone();
                        }
                        matches
                    })
            })
            .collect();
        if positions.is_empty() {
            continue;
        }
        // The cells showing those positions (the rest aren't built).
        let mut cell = grid.first_child();
        while let Some(current) = cell {
            cell = current.next_sibling();
            if let Some(root) = current.first_child().and_downcast::<gtk::Box>()
                && grid_position(root.upcast_ref()).is_some_and(|p| positions.contains(&p))
            {
                Card::from_root(&root).bind(ui, item, shape);
            }
        }
    }
}

const POSITION_KEY: &str = "embyclientplus-grid-position";

fn set_grid_position(root: &gtk::Box, position: u32) {
    // SAFETY: this key is only ever stored and read as `u32`.
    unsafe { root.set_data(POSITION_KEY, position) };
}

fn grid_position(widget: &gtk::Widget) -> Option<u32> {
    // SAFETY: see `set_grid_position`.
    unsafe { widget.data::<u32>(POSITION_KEY).map(|p| *p.as_ref()) }
}

/// Whether `widget` sits in a popover. Those hang off the widget they were
/// opened from (a card in the grid), so grid rules would reach out of them.
fn in_popover(widget: &impl IsA<gtk::Widget>) -> bool {
    widget.ancestor(gtk::Popover::static_type()).is_some()
}

/// Moves focus inside a grid by item position (GTK's directional focus
/// doesn't step through grid cells). Returns false when `focus` isn't in
/// a grid, or the move would leave it (the caller then moves normally).
pub fn move_in_grid(focus: &gtk::Widget, direction: gtk::DirectionType) -> bool {
    if in_popover(focus) {
        return false;
    }
    let Some(grid) = focus
        .ancestor(gtk::GridView::static_type())
        .and_downcast::<gtk::GridView>()
    else {
        return false;
    };
    // The focused cell, or the card inside it, carries the position.
    let cell = if grid_position(focus).is_some() {
        Some(focus.clone())
    } else {
        focus.first_child()
    };
    let (Some(cell), Some(model)) = (cell, grid.model()) else {
        return false;
    };
    let Some(position) = grid_position(&cell) else {
        return false;
    };
    let cell_width = cell.parent().map_or(cell.width(), |c| c.width()).max(1);
    let columns = i64::from((grid.width() / cell_width).max(1));
    let target = i64::from(position)
        + match direction {
            gtk::DirectionType::Left => -1,
            gtk::DirectionType::Right => 1,
            gtk::DirectionType::Up => -columns,
            gtk::DirectionType::Down => columns,
            _ => return false,
        };
    if target < 0 || target >= i64::from(model.n_items()) {
        return false;
    }
    grid.scroll_to(target as u32, gtk::ListScrollFlags::FOCUS, None);
    true
}

/// The bar above the grid holding `focus` (count, filter, sort), if the
/// grid has one: what Up from its top row goes to.
pub fn grid_header(focus: &gtk::Widget) -> Option<gtk::Widget> {
    if in_popover(focus) {
        return None;
    }
    let scrolled = focus.ancestor(gtk::GridView::static_type())?.parent()?;
    // A grid with an A-Z strip sits in a body box next to it.
    let block = scrolled
        .parent()
        .filter(|parent| parent.has_css_class(GRID_BODY_CLASS))
        .unwrap_or(scrolled);
    block
        .prev_sibling()
        .filter(|bar| bar.is_visible() && bar.has_css_class(GRID_BAR_CLASS))
}

/// Marks a grid's bar (see `grid_header`).
pub const GRID_BAR_CLASS: &str = "grid-bar";

/// Marks the box holding a grid's scroller and its A-Z strip.
pub const GRID_BODY_CLASS: &str = "grid-body";

/// Marks an A-Z strip (see `az_strip`).
pub const AZ_STRIP_CLASS: &str = "az-strip";

/// Slots in the strip: `#`, then A to Z.
const AZ_SLOTS: usize = 27;

/// What an item sorts under: Emby's `SortName` when it was asked for.
fn sort_key(item: &BaseItem) -> &str {
    item.sort_name
        .as_deref()
        .filter(|name| !name.is_empty())
        .unwrap_or(&item.name)
}

/// The strip slot a name falls under: 0 is `#` (anything not starting with
/// A to Z), 1 to 26 are the letters.
fn az_slot(name: &str) -> usize {
    match name.chars().next().map(|c| c.to_ascii_uppercase()) {
        Some(c @ 'A'..='Z') => 1 + usize::from(c as u8 - b'A'),
        _ => 0,
    }
}

fn az_label(slot: usize) -> String {
    match slot {
        0 => "#".to_string(),
        n => char::from(b'A' + (n - 1) as u8).to_string(),
    }
}

/// Which strip slots have at least one item in `model`.
fn az_present(model: &gio::ListModel) -> [bool; AZ_SLOTS] {
    let mut present = [false; AZ_SLOTS];
    for position in 0..model.n_items() {
        if let Some(object) = model.item(position).and_downcast::<glib::BoxedAnyObject>() {
            present[az_slot(sort_key(&object.borrow::<BaseItem>()))] = true;
        }
    }
    present
}

/// The first item (in the grid's order, whichever way it runs) under `slot`.
fn first_in_slot(model: &gio::ListModel, slot: usize) -> Option<u32> {
    (0..model.n_items()).find(|&position| {
        model
            .item(position)
            .and_downcast::<glib::BoxedAnyObject>()
            .is_some_and(|object| az_slot(sort_key(&object.borrow::<BaseItem>())) == slot)
    })
}

/// A narrow vertical strip of `#`, A-Z buttons for jumping around a
/// name-sorted `grid`; letters nothing starts with are dimmed. It scrolls
/// (no scrollbar) when a small window can't fit all 27. Hide it when the
/// grid isn't name-sorted.
pub fn az_strip(grid: &gtk::GridView, store: &gio::ListStore) -> gtk::ScrolledWindow {
    let letters = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for slot in 0..AZ_SLOTS {
        let button = gtk::Button::builder()
            .label(az_label(slot))
            .css_classes(["flat", "az-letter"])
            .build();
        let (grid, store) = (grid.downgrade(), store.downgrade());
        button.connect_clicked(move |_| {
            if let (Some(grid), Some(store)) = (grid.upgrade(), store.upgrade())
                && let Some(position) = first_in_slot(store.upcast_ref(), slot)
            {
                focus_cell(&grid, position);
            }
        });
        letters.append(&button);
    }
    let refresh = |letters: &gtk::Box, model: &gio::ListModel| {
        let mut button = letters.first_child();
        for present in az_present(model) {
            let Some(current) = button else { break };
            current.set_sensitive(present);
            button = current.next_sibling();
        }
    };
    refresh(&letters, store.upcast_ref());
    let weak = letters.downgrade();
    store.connect_items_changed(move |store, _, _, _| {
        if let Some(letters) = weak.upgrade() {
            refresh(&letters, store.upcast_ref());
        }
    });
    // Our own viewport, so `strip_buttons` knows what's inside.
    let viewport = gtk::Viewport::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    viewport.set_child(Some(&letters));
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .propagate_natural_height(true)
        .valign(gtk::Align::Center)
        .margin_end(6)
        .css_classes([AZ_STRIP_CLASS])
        .child(&viewport)
        .build()
}

/// A grid and its A-Z strip side by side (see `grid_header`).
pub fn grid_body(scrolled: &gtk::ScrolledWindow, strip: &gtk::ScrolledWindow) -> gtk::Box {
    scrolled.set_hexpand(true);
    let body = gtk::Box::builder()
        .vexpand(true)
        .css_classes([GRID_BODY_CLASS])
        .build();
    body.append(scrolled);
    body.append(strip);
    body
}

/// The strip next to `grid`, if it has one showing.
fn az_strip_of(grid: &gtk::GridView) -> Option<gtk::ScrolledWindow> {
    grid.parent()?
        .next_sibling()
        .filter(|strip| strip.is_visible() && strip.has_css_class(AZ_STRIP_CLASS))
        .and_downcast()
}

fn strip_buttons(strip: &gtk::ScrolledWindow) -> Vec<gtk::Widget> {
    let mut buttons = Vec::new();
    let mut next = strip
        .child()
        .and_downcast::<gtk::Viewport>()
        .and_then(|viewport| viewport.child())
        .and_then(|letters| letters.first_child());
    while let Some(button) = next {
        next = button.next_sibling();
        buttons.push(button);
    }
    buttons
}

const RETURN_KEY: &str = "embyclientplus-strip-return";

/// The letter nearest `slot` among the `sensitive` ones (the earlier on a tie).
fn nearest_sensitive(sensitive: &[bool], slot: usize) -> Option<usize> {
    (0..sensitive.len())
        .filter(|&index| sensitive[index])
        .min_by_key(|index| index.abs_diff(slot))
}

/// The next letter with items after (`forward`) or before `current`; none
/// at the ends, where the cursor stays.
fn step_sensitive(sensitive: &[bool], current: usize, forward: bool) -> Option<usize> {
    if forward {
        (current + 1..sensitive.len()).find(|&index| sensitive[index])
    } else {
        (0..current).rev().find(|&index| sensitive[index])
    }
}

fn sensitivity(buttons: &[gtk::Widget]) -> Vec<bool> {
    buttons.iter().map(|button| button.is_sensitive()).collect()
}

/// Moves focus into `strip`, onto the letter for `slot` (or the nearest
/// one with items), remembering the grid `position` to come back to.
/// False when no letter can take it.
fn enter_strip(strip: &gtk::ScrolledWindow, position: u32, slot: usize) -> bool {
    if in_popover(strip) {
        return false;
    }
    let buttons = strip_buttons(strip);
    let Some(target) = nearest_sensitive(&sensitivity(&buttons), slot).map(|i| &buttons[i]) else {
        return false;
    };
    // SAFETY: this key is only ever stored and read as `u32`.
    unsafe { strip.set_data(RETURN_KEY, position) };
    target.grab_focus()
}

/// The strip holding `focus`, if it's on one of its letters.
fn strip_of(focus: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    if in_popover(focus) {
        return None;
    }
    focus
        .ancestor(gtk::ScrolledWindow::static_type())
        .filter(|strip| strip.has_css_class(AZ_STRIP_CLASS))
        .and_downcast()
}

/// Puts focus back on the grid card the strip was entered from (or the
/// first one, if that's gone).
fn return_to_grid(strip: &gtk::ScrolledWindow) {
    let Some(grid) = strip
        .prev_sibling()
        .and_downcast::<gtk::ScrolledWindow>()
        .and_then(|scrolled| scrolled.child())
        .and_downcast::<gtk::GridView>()
    else {
        return;
    };
    let items = grid.model().map_or(0, |model| model.n_items());
    if items == 0 {
        grid.grab_focus();
        return;
    }
    // SAFETY: see `enter_strip`.
    let back = unsafe { strip.data::<u32>(RETURN_KEY).map(|p| *p.as_ref()) };
    focus_cell(&grid, back.unwrap_or(0).min(items - 1));
}

/// Scrolls `grid` to `position` and focuses that cell, from anywhere:
/// `ListScrollFlags::FOCUS` alone only moves the focus when it's already
/// inside the grid (otherwise it just marks the cell), and the grid's
/// own grab_focus then lands on the marked cell.
fn focus_cell(grid: &gtk::GridView, position: u32) {
    grid.scroll_to(position, gtk::ListScrollFlags::FOCUS, None);
    if !grid.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN) {
        grid.grab_focus();
    }
}

/// The grid `focus` belongs to: the one it's a card of, or the one under
/// the bar it's on.
fn grid_of_focus(focus: &gtk::Widget) -> Option<gtk::GridView> {
    if let Some(grid) = focus.ancestor(gtk::GridView::static_type()) {
        return grid.downcast().ok();
    }
    let mut widget = Some(focus.clone());
    while let Some(current) = widget {
        if current.has_css_class(GRID_BAR_CLASS) {
            return current
                .next_sibling()
                .filter(|body| body.has_css_class(GRID_BODY_CLASS))
                .and_then(|body| body.first_child())
                .and_downcast::<gtk::ScrolledWindow>()
                .and_then(|scrolled| scrolled.child())
                .and_downcast();
        }
        widget = current.parent();
    }
    None
}

/// Whether the letters action does something with `focus`: it's in the
/// strip, or on a grid (or its bar) whose strip is showing.
pub fn letters_available(focus: &gtk::Widget) -> bool {
    !in_popover(focus)
        && (strip_of(focus).is_some()
            || grid_of_focus(focus).is_some_and(|grid| az_strip_of(&grid).is_some()))
}

/// Whether `focus` is on a letter of an A-Z strip.
pub fn in_strip(focus: &gtk::Widget) -> bool {
    strip_of(focus).is_some()
}

/// The letters action: from a grid (or its bar) into the strip, at the
/// focused card's letter; from the strip back to the grid. False when
/// there's no strip to work with.
pub fn jump_letters(focus: &gtk::Widget) -> bool {
    if in_popover(focus) {
        return false;
    }
    if let Some(strip) = strip_of(focus) {
        return_to_grid(&strip);
        return true;
    }
    let Some(grid) = grid_of_focus(focus) else {
        return false;
    };
    let Some(strip) = az_strip_of(&grid) else {
        return false;
    };
    let card = grid_position(focus)
        .map(|_| focus.clone())
        .or_else(|| focus.first_child());
    // On the bar: the first card's letter, and back to the first card.
    let (position, slot) = card
        .and_then(|card| grid_position(&card))
        .and_then(|position| {
            let item = grid
                .model()?
                .item(position)
                .and_downcast::<glib::BoxedAnyObject>()?;
            Some((position, az_slot(sort_key(&item.borrow::<BaseItem>()))))
        })
        .unwrap_or((0, 0));
    enter_strip(&strip, position, slot)
}

/// B in the strip: back to the grid, not a page back. False when `focus`
/// isn't in a strip.
pub fn leave_strip(focus: &gtk::Widget) -> bool {
    let Some(strip) = strip_of(focus) else {
        return false;
    };
    return_to_grid(&strip);
    true
}

/// Moves focus inside an A-Z strip: Up/Down step between letters that
/// have items (and stop at the ends), Left goes back to the grid (to where
/// it left from), Right does nothing: focus never leaves any other way.
/// Returns false when `focus` isn't in a strip.
pub fn move_in_strip(focus: &gtk::Widget, direction: gtk::DirectionType) -> bool {
    let Some(strip) = strip_of(focus) else {
        return false;
    };
    let buttons = strip_buttons(&strip);
    let Some(current) = buttons.iter().position(|button| button == focus) else {
        return false;
    };
    let step = |forward| step_sensitive(&sensitivity(&buttons), current, forward);
    match direction {
        gtk::DirectionType::Up => {
            if let Some(next) = step(false) {
                buttons[next].grab_focus();
            }
        }
        gtk::DirectionType::Down => {
            if let Some(next) = step(true) {
                buttons[next].grab_focus();
            }
        }
        gtk::DirectionType::Left => return_to_grid(&strip),
        // Nowhere to go; stay put rather than jump out of the strip.
        _ => {}
    }
    true
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

/// Gap between grid cards, the same as Home's rows (half on each side of
/// a cell, from the `poster-grid` CSS).
const GRID_GAP: i32 = super::rows::CARD_SPACING;

/// The grid's least margin at each side.
const GRID_MARGIN: i32 = 12;

/// Keeps `grid` at as many columns as fit its space, so each cell is just
/// a card plus the gap (a GridView otherwise shares the whole width out
/// among its columns, leaving wide gaps between posters), and centres
/// them by splitting what's left between its side margins. Margins, not
/// `halign`: an aligned GridView in a scroller gets measured against the
/// visible height and GTK warns on every frame.
///
/// Re-fits when the grid's width changes (its scroller's page size) or its
/// items do, not every frame: a tick callback kept the frame clock running
/// (battery) for as long as a grid was up.
fn fit_columns(grid: &gtk::GridView, shape: Shape) {
    let last = Rc::new(Cell::new((-1, u32::MAX)));
    let queued = Rc::new(Cell::new(false));
    // Applied at the next frame's start, not inside the allocation that
    // announced it (changing margins there would re-queue the layout).
    let schedule = Rc::new({
        let weak = grid.downgrade();
        move || {
            let Some(grid) = weak.upgrade() else { return };
            if queued.replace(true) {
                return;
            }
            let (last, queued) = (last.clone(), queued.clone());
            grid.add_tick_callback(move |grid, _| {
                queued.set(false);
                refit(grid, shape, &last);
                glib::ControlFlow::Break
            });
        }
    });
    let watch_width = {
        let schedule = schedule.clone();
        move |grid: &gtk::GridView| {
            if let Some(adjustment) = grid.hadjustment() {
                let schedule = schedule.clone();
                adjustment.connect_page_size_notify(move |_| schedule());
            }
        }
    };
    // The scroller hands over its adjustment when the grid is put in it.
    watch_width(grid);
    grid.connect_notify_local(Some("hadjustment"), {
        let schedule = schedule.clone();
        move |grid, _| {
            watch_width(grid);
            schedule();
        }
    });
    grid.connect_parent_notify({
        let schedule = schedule.clone();
        move |_| schedule()
    });
    if let Some(model) = grid.model() {
        model.connect_items_changed({
            let schedule = schedule.clone();
            move |_, _, _, _| schedule()
        });
    }
    schedule();
}

/// Sets `grid`'s columns and side margins for its parent's width and its
/// item count, unless they're what `last` already fitted.
fn refit(grid: &gtk::GridView, shape: Shape, last: &Cell<(i32, u32)>) {
    let Some(space) = grid.parent().map(|p| p.width()) else {
        return;
    };
    // A short list (three collections) centres too: no empty columns.
    let items = grid.model().map_or(0, |m| m.n_items());
    if (space, items) != last.get() {
        last.set((space, items));
        let cell = (super::scaled(shape.size().0) + GRID_GAP).max(1);
        let room = (space - 2 * GRID_MARGIN).max(cell);
        let fit = (room / cell).clamp(1, 64);
        let columns = fit.min(i32::try_from(items).unwrap_or(i32::MAX).max(1));
        let side = GRID_MARGIN + (room - columns * cell) / 2;
        grid.set_margin_start(side);
        grid.set_margin_end(side);
        grid.set_min_columns(columns.min(2) as u32);
        grid.set_max_columns(columns as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fall_under_their_first_letter() {
        assert_eq!(az_slot("Akira"), 1);
        assert_eq!(az_slot("zodiac"), 26);
        assert_eq!(az_slot("12 Angry Men"), 0);
        assert_eq!(az_slot("[REC]"), 0);
        assert_eq!(az_slot("Émile"), 0);
        assert_eq!(az_slot(""), 0);
        assert_eq!(az_label(0), "#");
        assert_eq!(az_label(1), "A");
        assert_eq!(az_label(26), "Z");
    }

    #[test]
    fn sort_name_wins_over_name() {
        let item = |name: &str, sort: Option<&str>| BaseItem {
            name: name.into(),
            sort_name: sort.map(Into::into),
            ..Default::default()
        };
        assert_eq!(sort_key(&item("The Matrix", Some("matrix"))), "matrix");
        assert_eq!(sort_key(&item("The Matrix", None)), "The Matrix");
        assert_eq!(sort_key(&item("The Matrix", Some(""))), "The Matrix");
    }

    #[test]
    fn letters_step_over_empty_ones_and_stop_at_the_ends() {
        let sensitive = [false, true, false, false, true, true, false];
        assert_eq!(nearest_sensitive(&sensitive, 0), Some(1));
        assert_eq!(nearest_sensitive(&sensitive, 3), Some(4));
        assert_eq!(nearest_sensitive(&sensitive, 6), Some(5));
        assert_eq!(nearest_sensitive(&[false, false], 1), None);
        assert_eq!(step_sensitive(&sensitive, 1, true), Some(4));
        assert_eq!(step_sensitive(&sensitive, 5, false), Some(4));
        assert_eq!(step_sensitive(&sensitive, 5, true), None);
        assert_eq!(step_sensitive(&sensitive, 1, false), None);
    }
}
