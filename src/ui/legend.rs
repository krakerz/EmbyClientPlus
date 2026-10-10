//! The controller legend: a small bar at the bottom left listing what the
//! buttons do right now (Select, Back, Options, ...), using the user's own
//! mapping. Shown once the controller is used (hidden again on mouse or
//! touch input), never in the player. It stays at the bottom left; when the
//! mini player leaves too little room, the hints scroll slowly back and
//! forth on one line (a marquee) instead of being cut off.

use std::cell::Cell;

use adw::prelude::*;

use super::Ui;
use crate::controller::{self, Action, Pad};

const MARGIN: i32 = 12;
/// The legend's own padding and border, left and right together.
const CHROME: i32 = 30;

thread_local! {
    static PAD_IN_USE: Cell<bool> = const { Cell::new(false) };
}

/// The controller was used last (not the mouse or touch screen): keyboard
/// hints don't apply.
pub fn pad_in_use() -> bool {
    PAD_IN_USE.get()
}

/// A key was pressed in the player: its hints apply again.
pub fn keyboard_used() {
    PAD_IN_USE.set(false);
}

pub struct Legend {
    root: gtk::Box,
    scroller: gtk::ScrolledWindow,
    hints: gtk::Box,
    /// The controller was used last (not the mouse or touch screen).
    in_use: Cell<bool>,
}

impl Legend {
    pub fn new() -> Self {
        let hints = gtk::Box::builder().spacing(14).build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .can_focus(false)
            .focusable(false)
            .child(&hints)
            .build();
        let root = gtk::Box::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .margin_start(MARGIN)
            .margin_bottom(MARGIN)
            .can_target(false)
            .visible(false)
            .css_classes(["legend"])
            .build();
        root.append(&scroller);
        super::marquee::attach(&scroller);
        Legend {
            root,
            scroller,
            hints,
            in_use: Cell::new(false),
        }
    }

    pub fn is_in_use(&self) -> bool {
        self.in_use.get()
    }

    /// Controller pressed: the hints apply. Mouse or touch: they don't.
    pub fn set_in_use(&self, in_use: bool) {
        self.in_use.set(in_use);
        PAD_IN_USE.set(in_use);
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Rebuilds the hints for what's on screen now.
    pub fn update(&self, window: &adw::ApplicationWindow, ui: Option<&Ui>) {
        let shown = self.in_use.get()
            && controller::connected().is_some()
            && ui.is_some_and(|ui| !ui.player_visible())
            && window.visible_dialog().is_none();
        self.root.set_visible(shown);
        let Some(ui) = ui.filter(|_| shown) else {
            return;
        };
        super::clear(&self.hints);
        let bindings = controller::bindings();
        let panel = ui.music_panel().filter(|panel| panel.is_open());
        // Room up to the mini player or music panel (bottom right).
        let sheet = ui.widget();
        // The mini player, or the open panel above it: both bottom right.
        let corner = if panel.is_some() {
            sheet.sheet()
        } else {
            sheet.bottom_bar()
        };
        // Not laid out yet right after it appears: use its set width.
        let bar_width = corner.map_or(0, |w| w.width().max(w.width_request()) + MARGIN);
        let window_width = self.root.parent().map_or(0, |p| p.width());
        let room = window_width - 2 * MARGIN - bar_width - CHROME;
        self.scroller
            .set_max_content_width(if room > 0 { room } else { -1 });

        let mut hints: Vec<(Vec<Pad>, &str)> = Vec::new();
        if let Some(panel) = &panel {
            if panel.moving().is_some() {
                let mut pads = bindings.buttons(Action::Up);
                pads.extend(bindings.buttons(Action::Down));
                hints.push((pads, "Move"));
                hints.push((bindings.buttons(Action::Activate), "Done"));
            } else {
                hints.push((bindings.buttons(Action::Activate), "Select"));
                hints.push((bindings.buttons(Action::MusicPlayPause), "Play/Pause"));
                let mut pads = bindings.buttons(Action::MusicPrevious);
                pads.extend(bindings.buttons(Action::MusicNext));
                hints.push((pads, "Track"));
                hints.push((bindings.buttons(Action::MoveInQueue), "Move in queue"));
                hints.push((bindings.buttons(Action::CloseMusic), "Library"));
                hints.push((bindings.buttons(Action::Back), "Close"));
            }
        } else {
            let page = ui.nav().visible_page();
            let tabs = page
                .as_ref()
                .filter(|page| super::tab_stepper(page).is_some())
                .map(super::tab_label);
            let at_home = page
                .as_ref()
                .is_some_and(|page| page.tag().as_deref() == Some("home"));
            let can_go_back = ui.nav().navigation_stack().n_items() > 1;
            hints.push((bindings.buttons(Action::Activate), "Select"));
            if can_go_back {
                hints.push((bindings.buttons(Action::Back), "Back"));
            }
            hints.push((bindings.buttons(Action::ContextMenu), "Options"));
            if let Some(label) = tabs {
                let mut pads = bindings.buttons(Action::PreviousTab);
                pads.extend(bindings.buttons(Action::NextTab));
                hints.push((pads, label));
            }
            if ui.music().is_active() {
                hints.push((bindings.buttons(Action::OpenMusic), "Music"));
            }
            if !at_home {
                hints.push((bindings.buttons(Action::Home), "Home"));
            }
            hints.push((bindings.buttons(Action::Search), "Search"));
            hints.push((bindings.buttons(Action::Preferences), "Settings"));
        }

        for (pads, text) in hints {
            // An action the user left unmapped has nothing to show.
            if pads.is_empty() {
                continue;
            }
            let hint = gtk::Box::builder().spacing(6).build();
            for pad in pads {
                hint.append(
                    &gtk::Label::builder()
                        .label(glyph(pad))
                        .css_classes(["legend-key"])
                        .build(),
                );
            }
            hint.append(&gtk::Label::new(Some(text)));
            self.hints.append(&hint);
        }
    }
}

/// The short name printed on a button badge.
fn glyph(pad: Pad) -> &'static str {
    match pad {
        Pad::Select => "View",
        Pad::Start => "Menu",
        Pad::Up => "↑",
        Pad::Down => "↓",
        Pad::Left => "←",
        Pad::Right => "→",
        other => other.name(),
    }
}
