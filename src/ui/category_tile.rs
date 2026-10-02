//! Genre/tag tiles: a wide cover with the category name over it. While
//! hovered or focused the cover cycles through random titles from the
//! category.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::{Ui, fixed_picture, images};
use crate::emby::browse::{ImageRef, ItemQuery};
use crate::runtime::spawn_tokio;

const WIDTH: i32 = 300;
const HEIGHT: i32 = 169;
const COVERS: usize = 10;
const CYCLE: Duration = Duration::from_secs(2);
const TARGET_KEY: &str = "embyclientplus-category-target";

/// What a tile opens: its name and the query for its grid.
type Target = (String, ItemQuery);

struct Tile {
    stack: gtk::Stack,
    pictures: [gtk::Picture; 2],
    covers: RefCell<Vec<ImageRef>>,
    index: Cell<usize>,
    hovered: Cell<bool>,
    focused: Cell<bool>,
    timer: RefCell<Option<glib::SourceId>>,
    /// For loading upcoming covers.
    ui: RefCell<Option<super::WeakUi>>,
}

pub fn tile(ui: &Ui, name: &str, query: ItemQuery) -> gtk::FlowBoxChild {
    let pictures = [(); 2].map(|_| {
        gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .build()
    });
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(600)
        .build();
    stack.add_named(&fixed_picture(&pictures[0], WIDTH, HEIGHT), Some("0"));
    stack.add_named(&fixed_picture(&pictures[1], WIDTH, HEIGHT), Some("1"));

    let title = gtk::Label::builder()
        .label(name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["tile-name"])
        .build();
    let count = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["tile-count"])
        .build();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::End)
        .margin_start(14)
        .margin_end(14)
        .margin_bottom(12)
        .build();
    text.append(&title);
    text.append(&count);
    let shade = gtk::Box::builder().css_classes(["tile-shade"]).build();
    let frame = gtk::Overlay::builder()
        .child(&stack)
        .overflow(gtk::Overflow::Hidden)
        .halign(gtk::Align::Center)
        .css_classes(["card", "category-tile"])
        .build();
    frame.add_overlay(&shade);
    frame.add_overlay(&text);

    let child = gtk::FlowBoxChild::builder().child(&frame).build();
    let target: Target = (name.to_string(), query.clone());
    // SAFETY: this key is only ever stored and read as `Target`.
    unsafe { child.set_data(TARGET_KEY, target) };

    let tile = Rc::new(Tile {
        stack,
        pictures,
        covers: RefCell::new(Vec::new()),
        index: Cell::new(0),
        hovered: Cell::new(false),
        focused: Cell::new(false),
        timer: RefCell::new(None),
        ui: RefCell::new(None),
    });

    let motion = gtk::EventControllerMotion::new();
    motion.connect_enter({
        let tile = tile.clone();
        move |_, _, _| {
            tile.hovered.set(true);
            tile.update_cycling();
        }
    });
    motion.connect_leave({
        let tile = tile.clone();
        move |_| {
            tile.hovered.set(false);
            tile.update_cycling();
        }
    });
    frame.add_controller(motion);
    let focus = gtk::EventControllerFocus::new();
    focus.connect_enter({
        let tile = tile.clone();
        move |_| {
            tile.focused.set(true);
            tile.update_cycling();
        }
    });
    focus.connect_leave({
        let tile = tile.clone();
        move |_| {
            tile.focused.set(false);
            tile.update_cycling();
        }
    });
    child.add_controller(focus);
    child.connect_unmap({
        let tile = tile.clone();
        move |_| tile.stop()
    });

    load_covers(ui, &tile, &count, query);
    child
}

/// The name and grid query of an activated tile.
pub fn target(child: &gtk::FlowBoxChild) -> Option<Target> {
    // SAFETY: see `tile`; cloned out immediately.
    unsafe { child.data::<Target>(TARGET_KEY).map(|t| t.as_ref().clone()) }
}

/// A random handful of titles from the category: their art becomes the
/// covers, and the total becomes the count.
fn load_covers(ui: &Ui, tile: &Rc<Tile>, count: &gtk::Label, mut query: ItemQuery) {
    query.sort_by = Some("Random");
    query.limit = COVERS;
    let client = ui.client();
    let user_id = ui.user_id();
    let weak_ui = ui.downgrade();
    let tile = Rc::downgrade(tile);
    let count = count.downgrade();
    glib::spawn_future_local(async move {
        let result = spawn_tokio(async move { client.items(&user_id, &query).await }).await;
        let (Some(ui), Some(tile), Some(count)) =
            (weak_ui.upgrade(), tile.upgrade(), count.upgrade())
        else {
            return;
        };
        let page = match result {
            Ok(page) => page,
            Err(e) => {
                tracing::debug!("category covers failed: {e:#}");
                return;
            }
        };
        count.set_label(&match page.total_record_count {
            1 => "1 title".to_string(),
            n => format!("{n} titles"),
        });
        let covers: Vec<ImageRef> = page
            .items
            .iter()
            .filter_map(|item| {
                item.backdrop()
                    .or_else(|| item.landscape())
                    .or_else(|| item.poster())
            })
            .collect();
        if let Some(first) = covers.first() {
            images::load(
                &ui,
                &tile.pictures[0],
                Some(first.clone()),
                WIDTH as u32 * 2,
            );
        }
        if let Some(second) = covers.get(1) {
            images::load(
                &ui,
                &tile.pictures[1],
                Some(second.clone()),
                WIDTH as u32 * 2,
            );
        }
        tile.covers.replace(covers);
        tile.ui.replace(Some(ui.downgrade()));
    });
}

impl Tile {
    fn update_cycling(self: &Rc<Self>) {
        if self.hovered.get() || self.focused.get() {
            if self.timer.borrow().is_none() {
                let weak = Rc::downgrade(self);
                let timer = glib::timeout_add_local(CYCLE, move || match weak.upgrade() {
                    Some(tile) => {
                        tile.advance();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                });
                self.timer.replace(Some(timer));
            }
        } else {
            self.stop();
        }
    }

    fn stop(&self) {
        if let Some(timer) = self.timer.take() {
            timer.remove();
        }
    }

    /// Shows the preloaded next cover, then preloads the one after it into
    /// the picture that just went out of view, once the crossfade is over
    /// (loading it during the fade flashed a random cover).
    fn advance(self: &Rc<Self>) {
        let len = self.covers.borrow().len();
        if len < 2 || self.stack.is_transition_running() {
            return;
        }
        let next = (self.index.get() + 1) % len;
        self.index.set(next);
        let showing = next % 2;
        self.stack.set_visible_child_name(&showing.to_string());
        let weak = Rc::downgrade(self);
        let after_fade = Duration::from_millis(u64::from(self.stack.transition_duration()) + 100);
        glib::timeout_add_local_once(after_fade, move || {
            let Some(tile) = weak.upgrade() else { return };
            let upcoming = tile.covers.borrow()[(next + 1) % len].clone();
            if let Some(ui) = tile.ui.borrow().as_ref().and_then(|ui| ui.upgrade()) {
                images::load(
                    &ui,
                    &tile.pictures[1 - showing],
                    Some(upcoming),
                    WIDTH as u32 * 2,
                );
            }
        });
    }
}
