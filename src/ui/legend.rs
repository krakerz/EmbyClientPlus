//! The controller legend: a small bar at the bottom left listing what the
//! buttons do right now (Select, Back, Options, ...), using the user's own
//! mapping. Shown while a controller is connected, never in the player.

use adw::prelude::*;

use super::Ui;
use crate::controller::{self, Action, Pad};

const MARGIN: i32 = 12;

pub struct Legend {
    root: gtk::Box,
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
            .css_classes(["osd", "legend"])
            .build();
        Legend { root }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Rebuilds the hints for what's on screen now.
    pub fn update(&self, window: &adw::ApplicationWindow, ui: Option<&Ui>) {
        let shown = controller::connected().is_some()
            && ui.is_some_and(|ui| !ui.player_visible())
            && window.visible_dialog().is_none();
        self.root.set_visible(shown);
        let Some(ui) = ui.filter(|_| shown) else {
            return;
        };
        super::clear(&self.root);
        // Sit above the music mini player when it's showing.
        let sheet = ui.widget();
        let bar = if sheet.bottom_bar().is_some() {
            sheet.bottom_bar_height()
        } else {
            0
        };
        self.root.set_margin_bottom(MARGIN + bar);
        let bindings = controller::bindings();
        let page = ui.nav().visible_page();
        let tabs = page
            .as_ref()
            .filter(|page| super::tab_stepper(page).is_some())
            .map(super::tab_label);
        let at_home = page
            .as_ref()
            .is_some_and(|page| page.tag().as_deref() == Some("home"));
        let can_go_back = ui.nav().navigation_stack().n_items() > 1;

        let mut hints: Vec<(Vec<Pad>, &str)> = vec![(bindings.buttons(Action::Activate), "Select")];
        if can_go_back {
            hints.push((bindings.buttons(Action::Back), "Back"));
        }
        hints.push((bindings.buttons(Action::ContextMenu), "Options"));
        if let Some(label) = tabs {
            let mut pads = bindings.buttons(Action::PreviousTab);
            pads.extend(bindings.buttons(Action::NextTab));
            hints.push((pads, label));
        }
        if !at_home {
            hints.push((bindings.buttons(Action::Home), "Home"));
        }
        hints.push((bindings.buttons(Action::Search), "Search"));
        hints.push((bindings.buttons(Action::Preferences), "Settings"));

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
