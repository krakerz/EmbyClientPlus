//! Gamepad input: a background thread reads controllers with gilrs and
//! hands presses to the GTK main thread, where they're turned into
//! actions through the user's [`Bindings`].

pub mod bindings;

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use gilrs::{Axis, Button, EventType, Gilrs};
use gtk::glib;

use bindings::Repeater;
pub use bindings::{Action, Bindings, Context, Pad};

use crate::config::Settings;

/// Stick deflection that counts as a direction press, and the lower level
/// it must fall back under to release (so it doesn't flicker).
const STICK_PRESS: f32 = 0.5;
const STICK_RELEASE: f32 = 0.3;
const POLL: Duration = Duration::from_millis(16);

/// What the input thread reports.
#[derive(Debug, Clone, PartialEq)]
pub enum PadEvent {
    Pressed(Pad),
    /// A held direction repeating (see `Repeater`).
    Repeated(Pad),
    Connected(String),
    Disconnected,
}

/// Gets each press; `true` for a held direction's repeats.
type Handler = Box<dyn Fn(Pad, bool)>;
type Capture = Box<dyn FnOnce(Pad)>;

thread_local! {
    static HUB: Hub = Hub::default();
}

/// Main-thread state: who gets presses, and the current mapping.
#[derive(Default)]
struct Hub {
    handler: RefCell<Option<Handler>>,
    /// A one-shot "press a button…" listener (Preferences remapping).
    capture: RefCell<Option<Capture>>,
    connected: RefCell<Option<String>>,
    bindings: RefCell<Bindings>,
    started: Cell<bool>,
}

/// Starts reading controllers (once); presses go to `handler`.
pub fn start(handler: impl Fn(Pad, bool) + 'static) {
    HUB.with(|hub| {
        hub.handler.replace(Some(Box::new(handler)));
        hub.bindings.replace(load_bindings());
        if hub.started.replace(true) {
            return;
        }
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("gamepad".into())
            .spawn(move || read_pads(&sender))
            .expect("failed to start the gamepad thread");
        glib::spawn_future_local(async move {
            while let Some(event) = receiver.recv().await {
                HUB.with(|hub| hub.dispatch(event));
            }
        });
    });
}

impl Hub {
    fn dispatch(&self, event: PadEvent) {
        match event {
            PadEvent::Connected(name) => {
                tracing::info!("controller connected: {name}");
                self.connected.replace(Some(name));
            }
            PadEvent::Disconnected => {
                tracing::info!("controller disconnected");
                self.connected.replace(None);
            }
            PadEvent::Pressed(pad) => {
                if let Some(capture) = self.capture.take() {
                    capture(pad);
                } else if let Some(handler) = self.handler.borrow().as_ref() {
                    handler(pad, false);
                }
            }
            // Remapping waits for a real press.
            PadEvent::Repeated(pad) => {
                if self.capture.borrow().is_none()
                    && let Some(handler) = self.handler.borrow().as_ref()
                {
                    handler(pad, true);
                }
            }
        }
    }
}

/// The action `pad` maps to in `context`.
pub fn action_for(pad: Pad, context: Context) -> Option<Action> {
    HUB.with(|hub| hub.bindings.borrow().action_for(pad, context))
}

pub fn bindings() -> Bindings {
    HUB.with(|hub| hub.bindings.borrow().clone())
}

/// Saves a new mapping and uses it right away.
pub fn set_bindings(bindings: Bindings) {
    let config = bindings.to_config();
    if let Err(e) = Settings::update(|s| s.controller.bindings = config) {
        tracing::warn!("couldn't save controller bindings: {e:#}");
    }
    HUB.with(|hub| hub.bindings.replace(bindings));
}

/// Name of the connected controller, if any.
pub fn connected() -> Option<String> {
    HUB.with(|hub| hub.connected.borrow().clone())
}

/// Sends the next press to `callback` instead of the normal handler.
pub fn capture_next(callback: impl FnOnce(Pad) + 'static) {
    HUB.with(|hub| hub.capture.replace(Some(Box::new(callback))));
}

pub fn cancel_capture() {
    HUB.with(|hub| hub.capture.take());
}

fn load_bindings() -> Bindings {
    Bindings::from_config(&Settings::load().unwrap_or_default().controller.bindings)
}

/// The gamepad thread. Gilrs isn't `Send`, so it lives here entirely.
fn read_pads(sender: &tokio::sync::mpsc::UnboundedSender<PadEvent>) {
    let mut gilrs = match Gilrs::new() {
        Ok(gilrs) => gilrs,
        Err(e) => {
            tracing::warn!("controller support unavailable: {e}");
            return;
        }
    };
    let send = |event| sender.send(event).is_ok();
    if let Some((_, pad)) = gilrs.gamepads().next() {
        send(PadEvent::Connected(pad.name().to_string()));
    }
    let mut repeater = Repeater::default();
    let mut stick = StickState::default();
    loop {
        // One event per turn, so held directions still repeat while a
        // noisy stick keeps the queue busy.
        if let Some(event) = gilrs.next_event_blocking(Some(POLL)) {
            let now = Instant::now();
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    if let Some(pad) = pad_for(button) {
                        repeater.press(pad, now);
                        if !send(PadEvent::Pressed(pad)) {
                            return;
                        }
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    if let Some(pad) = pad_for(button) {
                        repeater.release(pad);
                    }
                }
                EventType::AxisChanged(axis, value, _) => {
                    for (pad, pressed) in stick.update(axis, value) {
                        if pressed {
                            repeater.press(pad, now);
                            if !send(PadEvent::Pressed(pad)) {
                                return;
                            }
                        } else {
                            repeater.release(pad);
                        }
                    }
                }
                EventType::Connected => {
                    let name = gilrs.gamepad(event.id).name().to_string();
                    send(PadEvent::Connected(name));
                }
                EventType::Disconnected => {
                    send(PadEvent::Disconnected);
                }
                _ => {}
            }
        }
        for pad in repeater.due(Instant::now()) {
            if !send(PadEvent::Repeated(pad)) {
                return;
            }
        }
    }
}

fn pad_for(button: Button) -> Option<Pad> {
    Some(match button {
        Button::South => Pad::A,
        Button::East => Pad::B,
        Button::West => Pad::X,
        Button::North => Pad::Y,
        Button::LeftTrigger => Pad::LB,
        Button::RightTrigger => Pad::RB,
        Button::LeftTrigger2 => Pad::LT,
        Button::RightTrigger2 => Pad::RT,
        Button::Select => Pad::Select,
        Button::Start => Pad::Start,
        Button::LeftThumb => Pad::L3,
        Button::RightThumb => Pad::R3,
        Button::DPadUp => Pad::Up,
        Button::DPadDown => Pad::Down,
        Button::DPadLeft => Pad::Left,
        Button::DPadRight => Pad::Right,
        _ => return None,
    })
}

/// Left stick as four virtual direction buttons, with hysteresis.
#[derive(Debug, Default)]
struct StickState {
    x: Option<Pad>,
    y: Option<Pad>,
}

impl StickState {
    /// (direction, pressed) changes caused by an axis move.
    fn update(&mut self, axis: Axis, value: f32) -> Vec<(Pad, bool)> {
        let (slot, negative, positive) = match axis {
            Axis::LeftStickX => (&mut self.x, Pad::Left, Pad::Right),
            // gilrs reports up as positive Y.
            Axis::LeftStickY => (&mut self.y, Pad::Down, Pad::Up),
            _ => return Vec::new(),
        };
        let wanted = if value <= -STICK_PRESS {
            Some(negative)
        } else if value >= STICK_PRESS {
            Some(positive)
        } else if value.abs() < STICK_RELEASE {
            None
        } else {
            *slot // between the thresholds: keep whatever it was
        };
        let mut changes = Vec::new();
        if wanted != *slot {
            if let Some(old) = *slot {
                changes.push((old, false));
            }
            if let Some(new) = wanted {
                changes.push((new, true));
            }
            *slot = wanted;
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stick_presses_with_hysteresis() {
        let mut stick = StickState::default();
        assert!(stick.update(Axis::LeftStickX, 0.4).is_empty());
        assert_eq!(stick.update(Axis::LeftStickX, 0.6), [(Pad::Right, true)]);
        // Wobbling between the thresholds doesn't re-trigger.
        assert!(stick.update(Axis::LeftStickX, 0.4).is_empty());
        assert!(stick.update(Axis::LeftStickX, 0.55).is_empty());
        assert_eq!(stick.update(Axis::LeftStickX, 0.1), [(Pad::Right, false)]);
        // Flicking straight across releases one side and presses the other.
        stick.update(Axis::LeftStickY, 0.9);
        assert_eq!(
            stick.update(Axis::LeftStickY, -0.9),
            [(Pad::Up, false), (Pad::Down, true)]
        );
        assert!(stick.update(Axis::RightStickX, 1.0).is_empty());
    }

    #[test]
    fn xbox_layout_maps_to_pad_names() {
        assert_eq!(pad_for(Button::South), Some(Pad::A));
        assert_eq!(pad_for(Button::North), Some(Pad::Y));
        assert_eq!(pad_for(Button::LeftTrigger), Some(Pad::LB));
        assert_eq!(pad_for(Button::RightTrigger2), Some(Pad::RT));
        assert_eq!(pad_for(Button::Mode), None);
    }
}
