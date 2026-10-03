//! Marquee text: a line that doesn't fit its (fixed) width scrolls slowly
//! back and forth, resting at each end, instead of being cut off. It only
//! moves while it overflows and is on screen.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

const TICK: Duration = Duration::from_millis(30);
const STEP: f64 = 1.0;
/// Ticks to rest at each end (1.5 s).
const REST_TICKS: u32 = 50;

/// `label` on one line inside a scroller that marquees it when it's too
/// long for the width it gets. Put the returned widget where the label
/// would go.
pub fn wrap(label: &gtk::Label) -> gtk::ScrolledWindow {
    wrap_inner(label, None)
}

/// Like [`wrap`], but it only moves while `owner` has the cursor or the
/// pointer (rows in long lists, so they don't all scroll at once).
pub fn wrap_while_active(label: &gtk::Label, owner: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    wrap_inner(label, Some(owner.as_ref()))
}

fn wrap_inner(label: &gtk::Label, owner: Option<&gtk::Widget>) -> gtk::ScrolledWindow {
    label.set_ellipsize(gtk::pango::EllipsizeMode::None);
    label.set_wrap(false);
    label.set_single_line_mode(true);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        // Its width comes from its parent; the text never widens it.
        .propagate_natural_width(false)
        .propagate_natural_height(true)
        .hexpand(true)
        // Text, not a place for the cursor: focus moves past it.
        .can_focus(false)
        .focusable(false)
        .child(label)
        .build();
    attach_inner(&scroller, owner);
    scroller
}

/// Makes `scroller`'s content marquee whenever it overflows.
pub fn attach(scroller: &gtk::ScrolledWindow) {
    attach_inner(scroller, None);
}

fn attach_inner(scroller: &gtk::ScrolledWindow, owner: Option<&gtk::Widget>) {
    let slot: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let owner_weak = owner.map(|o| o.downgrade());
    let update = {
        let slot = slot.clone();
        let weak = scroller.downgrade();
        move || {
            if let Some(scroller) = weak.upgrade() {
                let active = owner_weak.as_ref().is_none_or(|owner| {
                    owner.upgrade().is_some_and(|owner| {
                        owner
                            .state_flags()
                            .intersects(gtk::StateFlags::FOCUS_WITHIN | gtk::StateFlags::PRELIGHT)
                    })
                });
                if active {
                    update(&scroller, &slot);
                } else {
                    stop(&scroller, &slot);
                }
            }
        }
    };
    if let Some(owner) = owner {
        let update = update.clone();
        owner.connect_state_flags_changed(move |_, _| update());
    }
    let adjustment = scroller.hadjustment();
    for property in ["upper", "page-size"] {
        let update = update.clone();
        adjustment.connect_notify_local(Some(property), move |_, _| update());
    }
    let on_map = update.clone();
    scroller.connect_map(move |_| on_map());
    scroller.connect_unmap(move |scroller| stop(scroller, &slot));
}

fn overflow(scroller: &gtk::ScrolledWindow) -> f64 {
    let adjustment = scroller.hadjustment();
    adjustment.upper() - adjustment.page_size()
}

fn stop(scroller: &gtk::ScrolledWindow, slot: &RefCell<Option<glib::SourceId>>) {
    if let Some(source) = slot.take() {
        source.remove();
    }
    scroller.hadjustment().set_value(0.0);
}

fn update(scroller: &gtk::ScrolledWindow, slot: &Rc<RefCell<Option<glib::SourceId>>>) {
    if !scroller.is_mapped() || overflow(scroller) <= 1.0 {
        stop(scroller, slot);
        return;
    }
    if slot.borrow().is_some() {
        return; // running; it reads the size every tick
    }
    scroller.hadjustment().set_value(0.0);
    let rest = Cell::new(REST_TICKS);
    let forward = Cell::new(true);
    let weak = scroller.downgrade();
    let source = glib::timeout_add_local(TICK, move || {
        let Some(scroller) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        if rest.get() > 0 {
            rest.set(rest.get() - 1);
            return glib::ControlFlow::Continue;
        }
        let adjustment = scroller.hadjustment();
        let end = overflow(&scroller).max(0.0);
        let step = if forward.get() { STEP } else { -STEP };
        let value = adjustment.value() + step;
        if value >= end || value <= 0.0 {
            adjustment.set_value(value.clamp(0.0, end));
            forward.set(!forward.get());
            rest.set(REST_TICKS);
        } else {
            adjustment.set_value(value);
        }
        glib::ControlFlow::Continue
    });
    slot.replace(Some(source));
}
