//! Drag-to-scroll for every `ScrolledWindow` in the window, with momentum.
//!
//! One capture-phase controller on the window sees every press, so no
//! scroller needs wiring. Real touch is left to GTK's own kinetic scrolling.
//!
//! Dialogs (`adw::Dialog`) and popovers are not reached by the window's
//! controller: libadwaita dialogs and `GtkPopover`s deliver their input to
//! their own subtree. So `attach` also hooks every dialog the window
//! presents, and finds the popovers next to whatever gets pressed.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};

/// Pointer travel before a press turns into a drag (and stops counting as a
/// click, long press or button press underneath).
const DRAG_THRESHOLD: f64 = 8.0;
/// Scrollable range (px) a scroller needs on the dragged axis to take the drag.
const MIN_SCROLL_RANGE: f64 = 0.5;
/// Pointer samples older than this (µs) are dropped.
const SAMPLE_KEEP_US: i64 = 100_000;
/// Release speed (px/s) is measured over this span (µs) before the release;
/// a pointer that stood still for that long flings nothing.
const VELOCITY_WINDOW_US: i64 = 80_000;
/// Release speed (px/s) below which the content just stops.
const FLING_MIN_VELOCITY: f64 = 150.0;
/// Momentum ends once its speed (px/s) falls below this.
const STOP_VELOCITY: f64 = 20.0;
/// Time constant (s) of the momentum's exponential slowdown.
const DECAY_TIME: f64 = 0.325;
/// Longest frame gap (s) momentum integrates, so a stalled frame doesn't jump.
const MAX_FRAME_DT: f64 = 0.05;

thread_local! {
    /// The running momentum animation; at most one.
    static MOMENTUM: RefCell<Option<gtk::TickCallbackId>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// Horizontal when the drag is more sideways than vertical.
fn choose_axis(dx: f64, dy: f64) -> Axis {
    if dx.abs() > dy.abs() {
        Axis::Horizontal
    } else {
        Axis::Vertical
    }
}

/// Pointer velocity (px/s) along the drag axis at release time `now`, from
/// `(time µs, offset)` samples. Needs two samples inside the window.
fn release_velocity(samples: &[(i64, f64)], now: i64) -> f64 {
    let recent: Vec<&(i64, f64)> = samples
        .iter()
        .filter(|(t, _)| now - t <= VELOCITY_WINDOW_US)
        .collect();
    let (Some(first), Some(last)) = (recent.first(), recent.last()) else {
        return 0.0;
    };
    let span = last.0 - first.0;
    if recent.len() < 2 || span <= 0 {
        return 0.0;
    }
    (last.1 - first.1) / (span as f64 / 1_000_000.0)
}

/// One momentum frame: how far the pointer-equivalent moves in `dt` seconds
/// at speed `v`, and the slowed-down speed.
fn decay_step(v: f64, dt: f64) -> (f64, f64) {
    (v * dt, v * (-dt / DECAY_TIME).exp())
}

fn still_moving(v: f64) -> bool {
    v.abs() >= STOP_VELOCITY
}

fn adjustment(scroller: &gtk::ScrolledWindow, axis: Axis) -> gtk::Adjustment {
    match axis {
        Axis::Horizontal => scroller.hadjustment(),
        Axis::Vertical => scroller.vadjustment(),
    }
}

fn can_scroll(adj: &gtk::Adjustment) -> bool {
    adj.upper() - adj.page_size() - adj.lower() > MIN_SCROLL_RANGE
}

#[derive(Default)]
struct Drag {
    /// A sequence we may take over (left button, mouse, not on a slider).
    tracking: bool,
    /// Scrollers above the pressed widget, innermost first.
    chain: Vec<gtk::ScrolledWindow>,
    /// No scroller could take this drag; leave the sequence alone.
    declined: bool,
    /// The pressed widget, to look for popovers its click may create.
    picked: Option<glib::WeakRef<gtk::Widget>>,
    scroller: Option<(gtk::ScrolledWindow, Axis)>,
    start: f64,
    /// `(time µs, offset along the axis)`, newest last.
    samples: VecDeque<(i64, f64)>,
}

fn stop_momentum() {
    if let Some(id) = MOMENTUM.with_borrow_mut(Option::take) {
        id.remove();
    }
}

/// True for widgets whose own press/drag must keep working: sliders,
/// scrollbars, text input and anything that starts a drag-and-drop.
fn owns_drag(widget: &gtk::Widget) -> bool {
    if widget.is::<gtk::Range>()
        || widget.is::<gtk::Scrollbar>()
        || widget.is::<gtk::Editable>()
        || widget.is::<gtk::TextView>()
    {
        return true;
    }
    widget
        .observe_controllers()
        .iter::<glib::Object>()
        .flatten()
        .any(|controller| controller.is::<gtk::DragSource>())
}

/// Name of our controller, so a widget is never hooked twice.
const CONTROLLER_NAME: &str = "ecp-drag-scroll";

fn is_attached(widget: &gtk::Widget) -> bool {
    widget
        .observe_controllers()
        .iter::<glib::Object>()
        .flatten()
        .filter_map(|c| c.downcast::<gtk::EventController>().ok())
        .any(|c| c.name().is_some_and(|n| n == CONTROLLER_NAME))
}

/// Hooks every popover that hangs off `widget` or one of its ancestors (a
/// menu button's menu, a combo row's list, a card's context menu).
fn attach_popovers_around(widget: &gtk::Widget) {
    let mut current = Some(widget.clone());
    while let Some(w) = current {
        let mut child = w.first_child();
        while let Some(c) = child {
            if c.is::<gtk::Popover>() {
                attach(&c);
            }
            child = c.next_sibling();
        }
        current = w.parent();
    }
}

/// Makes every `ScrolledWindow` under `widget` scrollable by dragging. On an
/// application window it also covers every dialog it presents; popovers are
/// picked up as their opener is pressed. Safe to call twice on a widget.
pub fn attach(widget: &impl IsA<gtk::Widget>) {
    let window = widget.upcast_ref::<gtk::Widget>();
    if is_attached(window) {
        return;
    }
    if let Some(app_window) = window.downcast_ref::<adw::ApplicationWindow>() {
        let dialogs = app_window.dialogs();
        let hook = |model: &gtk::gio::ListModel, from: u32, count: u32| {
            for i in from..from + count {
                if let Some(dialog) = model.item(i).and_downcast::<gtk::Widget>() {
                    attach(&dialog);
                }
            }
        };
        hook(&dialogs, 0, dialogs.n_items());
        dialogs.connect_items_changed(move |model, pos, _, added| hook(model, pos, added));
        // The model only lives while somebody holds it: let the window do so.
        app_window.connect_destroy(move |_| drop(dialogs.clone()));
    }
    let drag = gtk::GestureDrag::builder()
        .button(gdk::BUTTON_PRIMARY)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .name(CONTROLLER_NAME)
        .build();
    let state = Rc::new(RefCell::new(Drag::default()));

    drag.connect_drag_begin({
        let state = state.clone();
        move |gesture, x, y| {
            stop_momentum();
            let mut st = state.borrow_mut();
            *st = Drag::default();
            if gesture
                .current_event_device()
                .is_some_and(|d| d.source() == gdk::InputSource::Touchscreen)
            {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            let Some(root) = gesture.widget() else { return };
            let Some(picked) = root.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return;
            };
            attach_popovers_around(&picked);
            st.picked = Some(picked.downgrade());
            let mut chain = Vec::new();
            let mut current = Some(picked);
            while let Some(widget) = current {
                if owns_drag(&widget) {
                    return;
                }
                if let Some(scroller) = widget.downcast_ref::<gtk::ScrolledWindow>() {
                    chain.push(scroller.clone());
                }
                current = widget.parent();
            }
            st.chain = chain;
            st.tracking = true;
        }
    });

    drag.connect_drag_update({
        let state = state.clone();
        move |gesture, dx, dy| {
            let mut st = state.borrow_mut();
            if !st.tracking || st.declined {
                return;
            }
            if st.scroller.is_none() {
                if dx.abs().max(dy.abs()) < DRAG_THRESHOLD {
                    return;
                }
                let axis = choose_axis(dx, dy);
                let found = st
                    .chain
                    .iter()
                    .find(|s| can_scroll(&adjustment(s, axis)))
                    .cloned();
                let Some(scroller) = found else {
                    st.declined = true;
                    return;
                };
                gesture.set_state(gtk::EventSequenceState::Claimed);
                scroller.set_cursor_from_name(Some("grabbing"));
                st.start = adjustment(&scroller, axis).value();
                st.scroller = Some((scroller, axis));
            }
            let Some((scroller, axis)) = st.scroller.clone() else {
                return;
            };
            let offset = match axis {
                Axis::Horizontal => dx,
                Axis::Vertical => dy,
            };
            adjustment(&scroller, axis).set_value(st.start - offset);
            let now = glib::monotonic_time();
            st.samples.push_back((now, offset));
            while st
                .samples
                .front()
                .is_some_and(|(t, _)| now - t > SAMPLE_KEEP_US)
            {
                st.samples.pop_front();
            }
        }
    });

    drag.connect_drag_end({
        let state = state.clone();
        move |_, _, _| {
            let mut st = state.borrow_mut();
            // A click may have just built a popover: hook it before it shows.
            if let Some(picked) = st.picked.take() {
                glib::idle_add_local_once(move || {
                    if let Some(widget) = picked.upgrade() {
                        attach_popovers_around(&widget);
                    }
                });
            }
            let Some((scroller, axis)) = st.scroller.take() else {
                return;
            };
            scroller.set_cursor(None::<&gdk::Cursor>);
            let samples: Vec<(i64, f64)> = st.samples.drain(..).collect();
            let velocity = release_velocity(&samples, glib::monotonic_time());
            st.tracking = false;
            if velocity.abs() > FLING_MIN_VELOCITY {
                start_momentum(&scroller, axis, velocity);
            }
        }
    });

    drag.connect_cancel({
        let state = state.clone();
        move |_, _| {
            let mut st = state.borrow_mut();
            if let Some((scroller, _)) = st.scroller.take() {
                scroller.set_cursor(None::<&gdk::Cursor>);
            }
            *st = Drag::default();
        }
    });

    window.add_controller(drag);
}

/// Keeps scrolling after release at the pointer's speed `velocity` (px/s,
/// along the drag), slowing down until it stops or hits an end.
fn start_momentum(scroller: &gtk::ScrolledWindow, axis: Axis, velocity: f64) {
    stop_momentum();
    let speed = std::cell::Cell::new(velocity);
    let last_frame = std::cell::Cell::new(None::<i64>);
    let id = scroller.add_tick_callback(move |widget, clock| {
        let finished = || {
            // The id is dead once we break; drop it without removing.
            MOMENTUM.with_borrow_mut(Option::take);
            glib::ControlFlow::Break
        };
        if !widget.is_mapped() {
            return finished();
        }
        let now = clock.frame_time();
        let Some(previous) = last_frame.replace(Some(now)) else {
            return glib::ControlFlow::Continue;
        };
        let dt = ((now - previous) as f64 / 1_000_000.0).clamp(0.0, MAX_FRAME_DT);
        let (distance, next) = decay_step(speed.get(), dt);
        speed.set(next);
        let Some(scroller) = widget.downcast_ref::<gtk::ScrolledWindow>() else {
            return finished();
        };
        let adj = adjustment(scroller, axis);
        let target = adj.value() - distance;
        let clamped = target.clamp(
            adj.lower(),
            (adj.upper() - adj.page_size()).max(adj.lower()),
        );
        adj.set_value(clamped);
        if clamped != target || !still_moving(next) {
            return finished();
        }
        glib::ControlFlow::Continue
    });
    MOMENTUM.with_borrow_mut(|slot| *slot = Some(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_follows_the_larger_travel() {
        assert_eq!(choose_axis(20.0, 3.0), Axis::Horizontal);
        assert_eq!(choose_axis(-20.0, 3.0), Axis::Horizontal);
        assert_eq!(choose_axis(3.0, -20.0), Axis::Vertical);
        assert_eq!(choose_axis(10.0, 10.0), Axis::Vertical);
    }

    #[test]
    fn velocity_from_recent_samples() {
        // 30 px in 30 ms, released right after: 1000 px/s.
        let samples = [(1_000_000, 0.0), (1_015_000, 15.0), (1_030_000, 30.0)];
        let v = release_velocity(&samples, 1_035_000);
        assert!((v - 1000.0).abs() < 1e-6, "got {v}");
        let up = [(1_000_000, 0.0), (1_030_000, -30.0)];
        assert!(release_velocity(&up, 1_031_000) < 0.0);
    }

    #[test]
    fn velocity_is_zero_when_the_pointer_stood_still() {
        let samples = [(1_000_000, 0.0), (1_030_000, 30.0)];
        assert_eq!(release_velocity(&samples, 1_500_000), 0.0);
        assert_eq!(release_velocity(&[], 1_000_000), 0.0);
    }

    #[test]
    fn velocity_is_zero_for_a_single_sample() {
        assert_eq!(release_velocity(&[(1_000_000, 30.0)], 1_010_000), 0.0);
    }

    #[test]
    fn momentum_slows_down_and_stops() {
        let (distance, next) = decay_step(1000.0, 0.016);
        assert!(distance > 0.0 && next < 1000.0 && next > 0.0);
        let (back, _) = decay_step(-1000.0, 0.016);
        assert!(back < 0.0);
        let mut v = 1000.0;
        let mut frames = 0;
        while still_moving(v) {
            v = decay_step(v, 0.016).1;
            frames += 1;
            assert!(frames < 1000, "momentum never stops");
        }
        assert!(!still_moving(10.0));
        assert!(still_moving(-500.0));
    }
}
