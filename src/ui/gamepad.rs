//! Turns controller presses into UI actions: focus movement and
//! activation while browsing, playback control on the player page.

use adw::prelude::*;

use super::{Ui, library, open_card_menu, preferences};
use crate::controller::{self, Action, Context, Pad};

/// Handles one press for `window`; `ui` is the logged-in UI, if any (the
/// login form only needs focus movement and activation).
pub fn handle(window: &adw::ApplicationWindow, ui: Option<&Ui>, pad: Pad) {
    // Controller use means focus rings matter, like keyboard use.
    window.set_focus_visible(true);

    if let Some(ui) = ui
        && ui.player_visible()
        && !ui.player_page().menu_open()
        && window.visible_dialog().is_none()
    {
        if let Some(action) = controller::action_for(pad, Context::Player) {
            ui.player_page().controller_action(action);
        }
        return;
    }
    let Some(action) = controller::action_for(pad, Context::Browse) else {
        return;
    };
    match action {
        Action::Up => move_focus(window, gtk::DirectionType::Up),
        Action::Down => move_focus(window, gtk::DirectionType::Down),
        Action::Left => move_focus(window, gtk::DirectionType::Left),
        Action::Right => move_focus(window, gtk::DirectionType::Right),
        Action::Activate => activate_focus(window),
        Action::Back => back(window, ui),
        Action::Home => {
            if let Some(ui) = ui {
                ui.nav().pop_to_tag("home");
            }
        }
        Action::ContextMenu => {
            if let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) {
                open_card_menu(&focus);
            }
        }
        Action::PreviousTab | Action::NextTab => {
            if let Some(ui) = ui {
                ui.step_tabs(action == Action::NextTab);
            }
        }
        Action::Search => {
            if let Some(ui) = ui {
                ui.open_search();
            }
        }
        Action::Preferences if ui.is_some() && window.visible_dialog().is_none() => {
            preferences::show(window);
        }
        _ => {}
    }
}

/// Moves focus like the arrow keys would, inside whichever surface holds
/// it (the window, an open popover, or a dialog).
fn move_focus(window: &adw::ApplicationWindow, direction: gtk::DirectionType) {
    let focus = gtk::prelude::GtkWindowExt::focus(window);
    if let Some(focus) = &focus
        && library::move_in_grid(focus, direction)
    {
        return;
    }
    let scope: gtk::Widget = focus
        .as_ref()
        .and_then(|widget| widget.native())
        .map(|native| native.upcast())
        .unwrap_or_else(|| window.clone().upcast());
    if focus.is_none() {
        scope.child_focus(gtk::DirectionType::TabForward);
    } else {
        scope.child_focus(direction);
    }
}

/// "Clicks" the focused widget.
fn activate_focus(window: &adw::ApplicationWindow) {
    let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) else {
        return;
    };
    if let Some(button) = focus.downcast_ref::<gtk::Button>() {
        button.emit_clicked();
    } else if let Some(row) = focus.downcast_ref::<gtk::ListBoxRow>() {
        row.emit_activate();
    } else if !focus.activate() {
        // Grid cells: the cell exposes activation as an action.
        let _ = focus.activate_action("listitem.activate", None);
    }
}

/// Closes the innermost thing open: a popover, a dialog, then a page.
fn back(window: &adw::ApplicationWindow, ui: Option<&Ui>) {
    if let Some(focus) = gtk::prelude::GtkWindowExt::focus(window)
        && let Some(popover) = focus
            .ancestor(gtk::Popover::static_type())
            .and_downcast::<gtk::Popover>()
    {
        popover.popdown();
        return;
    }
    if let Some(dialog) = window.visible_dialog() {
        dialog.close();
        return;
    }
    if let Some(ui) = ui {
        ui.nav().pop();
    }
}
