//! The right stick outside the player: scrolls what the cursor is in (its
//! row sideways, the page or dialog up and down), smoothly and faster the
//! further it's pushed. The cursor itself stays put; once it's scrolled out
//! of view, the next press brings it to the nearest visible item instead of
//! jumping back ([`refocus_if_scrolled_away`]).

use std::cell::{Cell, RefCell};

use gtk::glib;
use gtk::prelude::*;

use super::gamepad::usable;

/// Full-tilt speeds, in pixels per second at UI scale 1.
const ROW_SPEED: f64 = 2400.0;
const PAGE_SPEED: f64 = 2000.0;

thread_local! {
    /// The stick's position (x, y; y up).
    static TILT: Cell<(f64, f64)> = const { Cell::new((0.0, 0.0)) };
    static TICKING: RefCell<Option<gtk::TickCallbackId>> = const { RefCell::new(None) };
    /// The stick scrolled since the last press: the cursor may be off screen.
    static SCROLLED: Cell<bool> = const { Cell::new(false) };
}

/// The right stick's new position, for `window`.
pub fn set(window: &impl IsA<gtk::Widget>, x: f32, y: f32) {
    TILT.set((f64::from(x), f64::from(y)));
    if x == 0.0 && y == 0.0 {
        if let Some(id) = TICKING.take() {
            id.remove();
        }
        return;
    }
    if TICKING.with(|ticking| ticking.borrow().is_some()) {
        return;
    }
    // Per frame, so the scroll is as smooth as the display.
    let last = Cell::new(None::<i64>);
    let id = window.add_tick_callback(move |window, clock| {
        let now = clock.frame_time();
        let seconds = last
            .replace(Some(now))
            .map_or(0.0, |previous| (now - previous) as f64 / 1e6)
            .min(0.1);
        let (x, y) = TILT.get();
        scroll(window.upcast_ref(), x, y, seconds);
        glib::ControlFlow::Continue
    });
    TICKING.replace(Some(id));
}

/// Slow near the centre for fine control, full speed at the edge.
fn curve(tilt: f64) -> f64 {
    tilt * tilt.abs()
}

fn scroll(window: &gtk::Widget, x: f64, y: f64, seconds: f64) {
    let Some(focus) = window.root().and_then(|root| root.focus()) else {
        return;
    };
    let speed = super::ui_scale() * seconds;
    let mut moved = false;
    if x != 0.0
        && let Some(adjustment) = scrollable(&focus, gtk::Orientation::Horizontal)
    {
        moved |= nudge(&adjustment, curve(x) * ROW_SPEED * speed);
    }
    if y != 0.0
        && let Some(adjustment) = scrollable(&focus, gtk::Orientation::Vertical)
    {
        // Stick up scrolls up.
        moved |= nudge(&adjustment, -curve(y) * PAGE_SPEED * speed);
    }
    if moved {
        SCROLLED.set(true);
    }
}

/// The nearest scroller around `widget` with room to scroll that way.
fn scrollable(widget: &gtk::Widget, orientation: gtk::Orientation) -> Option<gtk::Adjustment> {
    let mut current = widget.ancestor(gtk::ScrolledWindow::static_type());
    while let Some(scroller) = current.and_downcast::<gtk::ScrolledWindow>() {
        let adjustment = match orientation {
            gtk::Orientation::Horizontal => scroller.hadjustment(),
            _ => scroller.vadjustment(),
        };
        if adjustment.upper() - adjustment.lower() > adjustment.page_size() + 1.0 {
            return Some(adjustment);
        }
        current = scroller
            .parent()
            .and_then(|parent| parent.ancestor(gtk::ScrolledWindow::static_type()));
    }
    None
}

/// Moves `adjustment` by `delta` within its range; true if it moved.
fn nudge(adjustment: &gtk::Adjustment, delta: f64) -> bool {
    let end = (adjustment.upper() - adjustment.page_size()).max(adjustment.lower());
    let value = (adjustment.value() + delta).clamp(adjustment.lower(), end);
    let moved = (value - adjustment.value()).abs() > f64::EPSILON;
    adjustment.set_value(value);
    moved
}

/// After the stick scrolled the cursor out of view: puts it on the visible
/// item nearest to where it was, and says so (the press is spent on that).
pub fn refocus_if_scrolled_away(window: &impl IsA<gtk::Widget>) -> bool {
    if !SCROLLED.replace(false) {
        return false;
    }
    let Some(root) = window.root() else {
        return false;
    };
    let Some(focus) = root.focus() else {
        return false;
    };
    if in_view(&focus) {
        return false;
    }
    let Some(area) = outermost_scroller(&focus) else {
        return false;
    };
    let root = root.upcast::<gtk::Widget>();
    let Some(origin) = centre(&focus, &root) else {
        return false;
    };
    let mut nearest: Option<(f32, gtk::Widget)> = None;
    visit(&area, &mut |widget| {
        if !usable(widget) || !in_view(widget) {
            return;
        }
        if let Some(at) = centre(widget, &root) {
            let distance = (at.0 - origin.0).powi(2) + (at.1 - origin.1).powi(2);
            if nearest.as_ref().is_none_or(|(best, _)| distance < *best) {
                nearest = Some((distance, widget.clone()));
            }
        }
    });
    nearest.is_some_and(|(_, widget)| widget.grab_focus())
}

/// Whether `widget`'s centre shows inside every scroller around it.
fn in_view(widget: &gtk::Widget) -> bool {
    let mut current = widget.ancestor(gtk::ScrolledWindow::static_type());
    while let Some(scroller) = current {
        let Some((x, y)) = centre(widget, &scroller) else {
            return false;
        };
        if x < 0.0 || y < 0.0 || x > scroller.width() as f32 || y > scroller.height() as f32 {
            return false;
        }
        current = scroller
            .parent()
            .and_then(|parent| parent.ancestor(gtk::ScrolledWindow::static_type()));
    }
    true
}

fn outermost_scroller(widget: &gtk::Widget) -> Option<gtk::Widget> {
    let mut outermost = None;
    let mut current = widget.ancestor(gtk::ScrolledWindow::static_type());
    while let Some(scroller) = current {
        current = scroller
            .parent()
            .and_then(|parent| parent.ancestor(gtk::ScrolledWindow::static_type()));
        outermost = Some(scroller);
    }
    outermost
}

fn centre(widget: &gtk::Widget, relative_to: &impl IsA<gtk::Widget>) -> Option<(f32, f32)> {
    let bounds = widget.compute_bounds(relative_to)?;
    Some((
        bounds.x() + bounds.width() / 2.0,
        bounds.y() + bounds.height() / 2.0,
    ))
}

/// Calls `found` on every shown widget under `widget`.
fn visit(widget: &gtk::Widget, found: &mut impl FnMut(&gtk::Widget)) {
    let mut child = widget.first_child();
    while let Some(current) = child {
        if current.is_mapped() {
            found(&current);
            visit(&current, found);
        }
        child = current.next_sibling();
    }
}

#[cfg(test)]
mod tests {
    use super::curve;

    #[test]
    fn the_curve_keeps_direction_and_is_gentle_near_centre() {
        assert_eq!(curve(1.0), 1.0);
        assert_eq!(curve(-1.0), -1.0);
        assert_eq!(curve(0.5), 0.25);
        assert_eq!(curve(-0.5), -0.25);
    }
}
