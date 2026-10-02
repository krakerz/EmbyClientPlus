//! Horizontal card rows with a "Title >" heading, used by Home and the
//! library Suggestions tab.

use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gdk;

use super::Ui;
use super::card::{self, Shape};
use super::library;
use crate::emby::models::BaseItem;

/// Pointer travel before a press on a card turns into a drag (and stops
/// counting as a click).
const DRAG_THRESHOLD: f64 = 8.0;

/// Gap between cards in a row.
pub const CARD_SPACING: i32 = 12;
/// Space before the first and after the last card.
pub const ROW_MARGIN: i32 = 18;

/// Where a row's title leads, shown as a "Title >" link.
pub enum More {
    None,
    /// Continue Watching, optionally within one library.
    Resume(Option<String>),
    /// Next Up, optionally within one library.
    NextUp(Option<String>),
    Library(Box<BaseItem>),
    /// A sortable grid of this query, titled.
    Grid(&'static str, Box<crate::emby::browse::ItemQuery>),
}

/// Items in a full Continue Watching / Next Up page.
const FULL_LIST_LIMIT: usize = 100;

fn open_more(ui: &Ui, more: &More) {
    let page = match more {
        More::None => return,
        More::Library(view) => return ui.open(view),
        More::Grid(title, query) => super::library_page::grid_page(ui, title, (**query).clone()),
        More::Resume(parent) => {
            let parent = parent.clone();
            library::list_page(
                ui,
                "Continue Watching",
                Shape::for_episodes(),
                |client, user_id| async move {
                    client
                        .resume(&user_id, parent.as_deref(), FULL_LIST_LIMIT)
                        .await
                },
            )
        }
        More::NextUp(parent) => {
            let parent = parent.clone();
            library::list_page(
                ui,
                "Next Up",
                Shape::for_episodes(),
                |client, user_id| async move {
                    client
                        .next_up(&user_id, parent.as_deref(), FULL_LIST_LIMIT)
                        .await
                },
            )
        }
    };
    ui.inner.nav.push(&page);
}

#[derive(Clone, Copy)]
pub enum Click {
    Open,
    /// Continue Watching cards start playback right away.
    Resume,
    /// Open in place of the current page (episode to episode).
    Replace,
}

pub fn row(
    ui: &Ui,
    title: &str,
    items: &[BaseItem],
    shape: Shape,
    click: Click,
    more: More,
) -> gtk::Box {
    let cards = gtk::Box::builder()
        .spacing(CARD_SPACING)
        .margin_start(ROW_MARGIN)
        .margin_end(ROW_MARGIN)
        .build();
    for item in items {
        let weak = ui.downgrade();
        let target = item.clone();
        cards.append(&card::button(ui, item, shape, move || {
            if let Some(ui) = weak.upgrade() {
                match click {
                    Click::Open => ui.open(&target),
                    Click::Resume => ui.play(&target, target.resume_ticks()),
                    Click::Replace => ui.open_replacing(&target),
                }
            }
        }));
    }
    let scroller = gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::Never)
        .child(&cards)
        .build();
    drag_to_scroll(&scroller);

    let section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    section.append(&section_title(ui, title, more));
    section.append(&scroller);
    section
}

/// Lets a mouse drag the row sideways, like a touch swipe. Runs in the
/// capture phase so it sees presses on the cards; once the pointer moves
/// past the threshold it claims the sequence, which cancels the card's click.
fn drag_to_scroll(scroller: &gtk::ScrolledWindow) {
    let drag = gtk::GestureDrag::builder()
        .button(gdk::BUTTON_PRIMARY)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    let start = Rc::new(Cell::new(0.0));
    let dragging = Rc::new(Cell::new(false));
    drag.connect_drag_begin({
        let scroller = scroller.clone();
        let start = start.clone();
        let dragging = dragging.clone();
        move |_, _, _| {
            start.set(scroller.hadjustment().value());
            dragging.set(false);
        }
    });
    drag.connect_drag_update({
        let scroller = scroller.clone();
        let dragging = dragging.clone();
        move |gesture, offset_x, _| {
            if !dragging.get() {
                if offset_x.abs() < DRAG_THRESHOLD {
                    return;
                }
                dragging.set(true);
                gesture.set_state(gtk::EventSequenceState::Claimed);
                scroller.set_cursor_from_name(Some("grabbing"));
            }
            scroller.hadjustment().set_value(start.get() - offset_x);
        }
    });
    drag.connect_drag_end({
        let scroller = scroller.clone();
        move |_, _, _| scroller.set_cursor(None::<&gdk::Cursor>)
    });
    scroller.add_controller(drag);
}

/// The row heading; a flat "Title >" button when it leads somewhere.
fn section_title(ui: &Ui, title: &str, more: More) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(title)
        .xalign(0.0)
        .css_classes(["title-3"])
        .build();
    if matches!(more, More::None) {
        label.set_margin_start(18);
        return label.upcast();
    }
    let content = gtk::Box::builder().spacing(6).build();
    content.append(&label);
    content.append(&gtk::Image::from_icon_name(crate::ui::icons::NEXT));
    let button = gtk::Button::builder()
        .child(&content)
        .halign(gtk::Align::Start)
        .margin_start(10)
        .css_classes(["flat", "section-link"])
        .build();
    let weak = ui.downgrade();
    button.connect_clicked(move |_| {
        if let Some(ui) = weak.upgrade() {
            open_more(&ui, &more);
        }
    });
    button.upcast()
}
