//! The controller legend: a small bar at the bottom left listing what the
//! buttons do right now (Select, Back, Options, ...), using the user's own
//! mapping. Shown once the controller is used (hidden again on mouse or
//! touch input), never in the player.

use adw::prelude::*;

use super::Ui;
use crate::controller::{self, Action, Pad};

const MARGIN: i32 = 12;

pub struct Legend {
    root: gtk::Box,
    /// The controller was used last (not the mouse or touch screen).
    in_use: std::cell::Cell<bool>,
}

impl Legend {
    pub fn new() -> Self {
        let root = gtk::Box::builder()
            .spacing(14)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .margin_start(12)
            .margin_bottom(MARGIN)
            .can_target(false)
            .visible(false)
            .css_classes(["legend"])
            .build();
        Legend {
            root,
            in_use: std::cell::Cell::new(false),
        }
    }

    pub fn is_in_use(&self) -> bool {
        self.in_use.get()
    }

    /// Controller pressed: the hints apply. Mouse or touch: they don't.
    pub fn set_in_use(&self, in_use: bool) {
        self.in_use.set(in_use);
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
        super::clear(&self.root);
        let bindings = controller::bindings();
        let panel = ui.music_panel().filter(|panel| panel.is_open());
        // Sit just above the music bar when it's showing (it spans the
        // width); otherwise at the very bottom.
        let sheet = ui.widget();
        let bar = if panel.is_none() && sheet.bottom_bar().is_some() {
            sheet.bottom_bar_height()
        } else {
            0
        };
        self.root.set_margin_bottom(MARGIN + bar);

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
            self.root.append(&hint);
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
