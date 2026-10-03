//! Turns controller presses into UI actions: focus movement and
//! activation while browsing, playback control on the player page.

use adw::prelude::*;
use gtk::glib;

use super::{Ui, library, open_card_menu, preferences};
use crate::controller::{self, Action, Context, Pad};

/// Handles one press for `window`; `ui` is the logged-in UI, if any (the
/// login form only needs focus movement and activation). `repeat`: a held
/// direction repeating rather than a fresh press.
pub fn handle(window: &adw::ApplicationWindow, ui: Option<&Ui>, pad: Pad, repeat: bool) {
    // Controller use means focus rings matter, like keyboard use.
    window.set_focus_visible(true);

    // The focused widget went away (a page reloaded, a grid rebuilt): put
    // the cursor back before acting, or the press lands on nothing.
    if let Some(ui) = ui
        && !ui.player_visible()
        && window.visible_dialog().is_none()
        && restore_focus(window, ui)
        && matches!(
            controller::action_for(pad, Context::Browse),
            Some(Action::Up | Action::Down | Action::Left | Action::Right)
        )
    {
        // A direction just brings the cursor back; it doesn't move it on.
        return;
    }

    if let Some(ui) = ui
        && ui.player_visible()
        && window.visible_dialog().is_none()
    {
        let page = ui.player_page();
        // Start: the OSD's buttons, driven like a page (directions move, A
        // presses or opens, B or Start leave). Menus opened there work as
        // below.
        if page.controls_mode() && !page.menu_open() {
            match controller::action_for(pad, Context::Browse) {
                Some(Action::Back | Action::Preferences) => page.leave_controls_mode(),
                Some(Action::Up) => move_focus(window, gtk::DirectionType::Up),
                Some(Action::Down) => move_focus(window, gtk::DirectionType::Down),
                Some(Action::Left) => horizontal_in(window, false),
                Some(Action::Right) => horizontal_in(window, true),
                Some(Action::Activate) => activate_focus(window),
                _ => {}
            }
            return;
        }
        if !page.menu_open() {
            if let Some(action) = controller::action_for(pad, Context::Player) {
                ui.player_page().controller_action(action, repeat);
            }
            return;
        }
        // A player menu is open: the controller stays in the player. It moves
        // through the menu, picks or closes it; any other button (like the
        // one that opened it) just closes it, and never leaves the player.
        match controller::action_for(pad, Context::Browse) {
            Some(
                Action::Up
                | Action::Down
                | Action::Left
                | Action::Right
                | Action::Activate
                | Action::Back,
            ) => {}
            _ => {
                close_popover(window);
                return;
            }
        }
    }
    let Some(action) = controller::action_for(pad, Context::Browse) else {
        return;
    };
    match action {
        Action::Up => move_focus(window, gtk::DirectionType::Up),
        Action::Down => move_focus(window, gtk::DirectionType::Down),
        Action::Left => horizontal_in(window, false),
        Action::Right => horizontal_in(window, true),
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
                ui.start_search();
            }
        }
        Action::Preferences if ui.is_some() && window.visible_dialog().is_none() => {
            preferences::show(window);
        }
        _ => {}
    }
}

const LAST_FOCUS_KEY: &str = "embyclientplus-last-focus";

/// Remembers, per page, the widget last focused there, for
/// [`restore_focus`].
pub fn track_focus(window: &adw::ApplicationWindow) {
    let debug = std::env::var_os("EMBYCLIENTPLUS_DEBUG_FOCUS").is_some();
    window.connect_focus_widget_notify(move |window| {
        let focus = gtk::prelude::GtkWindowExt::focus(window);
        if debug {
            tracing::info!("focus: {}", describe(focus.as_ref()));
        }
        let Some(focus) = focus else {
            return;
        };
        // Pages hand focus to their scroller (or a list to itself) when
        // they open; nothing there reacts to a press. Pass it on to the
        // first real item inside.
        if is_container(&focus) {
            let container = focus.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(container) = container.upgrade()
                    && container.has_focus()
                {
                    focus_first_inside(&container);
                }
            });
            return;
        }
        if let Some(page) = focus.ancestor(adw::NavigationPage::static_type()) {
            // SAFETY: only ever stored and read as `glib::WeakRef<gtk::Widget>`.
            unsafe { page.set_data(LAST_FOCUS_KEY, focus.downgrade()) };
        }
    });
}

/// Widgets that can hold focus but do nothing with a press.
fn is_container(widget: &gtk::Widget) -> bool {
    widget.is::<gtk::ScrolledWindow>()
        || widget.is::<gtk::Viewport>()
        || widget.is::<gtk::ListBox>()
        || widget.is::<adw::NavigationPage>()
        || widget.is::<adw::ToolbarView>()
}

/// Focuses the first focusable item inside `container`.
fn focus_first_inside(container: &gtk::Widget) -> bool {
    if let Some(list) = container.downcast_ref::<gtk::ListBox>()
        && let Some(row) = list.row_at_index(0)
    {
        return row.grab_focus();
    }
    let mut child = container.first_child();
    while let Some(current) = child {
        if current.child_focus(gtk::DirectionType::TabForward) {
            return true;
        }
        child = current.next_sibling();
    }
    false
}

/// "GtkButton.card-button < GtkBox < AdwClamp ..." for focus logging.
fn describe(widget: Option<&gtk::Widget>) -> String {
    let Some(widget) = widget else {
        return "none".to_string();
    };
    let one = |w: &gtk::Widget| {
        let classes = w.css_classes();
        let mut text = w.type_().name().to_string();
        for class in classes.iter().take(2) {
            text.push('.');
            text.push_str(class);
        }
        text
    };
    let mut chain = vec![format!(
        "{}{}",
        one(widget),
        if widget.is_mapped() {
            ""
        } else {
            " (unmapped)"
        }
    )];
    let mut parent = widget.parent();
    while let Some(p) = parent.take().filter(|_| chain.len() < 6) {
        chain.push(one(&p));
        parent = p.parent();
    }
    chain.join(" < ")
}

/// When nothing usable has focus (none, or a widget that's gone or hidden),
/// focuses the visible page's last focused widget, else its first item
/// below the header. True if it moved the focus.
fn restore_focus(window: &adw::ApplicationWindow, ui: &Ui) -> bool {
    let focus = gtk::prelude::GtkWindowExt::focus(window);
    if focus
        .as_ref()
        .is_some_and(|f| f.is_mapped() && f.is_sensitive() && !is_container(f))
    {
        return false;
    }
    let Some(page) = ui.nav().visible_page() else {
        return false;
    };
    // SAFETY: see `track_focus`; upgraded immediately.
    let last = unsafe {
        page.data::<glib::WeakRef<gtk::Widget>>(LAST_FOCUS_KEY)
            .and_then(|weak| weak.as_ref().upgrade())
    };
    if let Some(last) = last
        && last.is_mapped()
        && last.is_sensitive()
        && !is_container(&last)
        && last.grab_focus()
    {
        return true;
    }
    let content: Option<gtk::Widget> = page
        .child()
        .and_downcast::<adw::ToolbarView>()
        .and_then(|toolbar| toolbar.content())
        .or_else(|| page.child());
    content.is_some_and(|content| focus_first_inside(&content))
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

/// Left/Right: changes the focused setting's value, else moves focus.
fn horizontal_in(window: &adw::ApplicationWindow, forward: bool) {
    let focus = gtk::prelude::GtkWindowExt::focus(window);
    if !focus.as_ref().is_some_and(|f| adjust_value(f, forward)) {
        move_focus(
            window,
            if forward {
                gtk::DirectionType::Right
            } else {
                gtk::DirectionType::Left
            },
        );
    }
}

/// Left/Right on a setting changes its value instead of moving focus:
/// combo rows step through their options, spin rows and sliders step,
/// switches turn off/on. False when the focus isn't on such a widget.
fn adjust_value(focus: &gtk::Widget, forward: bool) -> bool {
    if let Some(combo) = focus
        .ancestor(adw::ComboRow::static_type())
        .and_downcast::<adw::ComboRow>()
    {
        let count = combo.model().map_or(0, |m| m.n_items());
        let selected = combo.selected();
        let next = match (forward, selected) {
            (_, gtk::INVALID_LIST_POSITION) => 0,
            (true, s) => (s + 1).min(count.saturating_sub(1)),
            (false, s) => s.saturating_sub(1),
        };
        if count > 0 {
            combo.set_selected(next);
        }
        return true;
    }
    if let Some(spin) = focus
        .ancestor(adw::SpinRow::static_type())
        .and_downcast::<adw::SpinRow>()
    {
        let adjustment = spin.adjustment();
        let step = adjustment.page_increment().max(adjustment.step_increment());
        adjustment.set_value(adjustment.value() + if forward { step } else { -step });
        return true;
    }
    if let Some(switch) = focus
        .ancestor(adw::SwitchRow::static_type())
        .and_downcast::<adw::SwitchRow>()
    {
        switch.set_active(forward);
        return true;
    }
    if let Some(scale) = focus.downcast_ref::<gtk::Scale>() {
        // Through the slider's own signal, so listeners see a user change.
        scale.emit_move_slider(if forward {
            gtk::ScrollType::PageRight
        } else {
            gtk::ScrollType::PageLeft
        });
        return true;
    }
    false
}

/// Closes the popover holding the focus, if any.
fn close_popover(window: &adw::ApplicationWindow) -> bool {
    if let Some(focus) = gtk::prelude::GtkWindowExt::focus(window)
        && let Some(popover) = focus
            .ancestor(gtk::Popover::static_type())
            .and_downcast::<gtk::Popover>()
    {
        popover.popdown();
        return true;
    }
    false
}

/// "Clicks" the focused widget.
fn activate_focus(window: &adw::ApplicationWindow) {
    let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) else {
        return;
    };
    // A, on a combo row, steps to its next option (wrapping), rather than
    // opening a drop-down list that's awkward to drive with a controller.
    if let Some(combo) = focus
        .ancestor(adw::ComboRow::static_type())
        .and_downcast::<adw::ComboRow>()
    {
        let count = combo.model().map_or(0, |m| m.n_items());
        if count > 0 {
            let selected = combo.selected();
            combo.set_selected(if selected == gtk::INVALID_LIST_POSITION {
                0
            } else {
                (selected + 1) % count
            });
        }
        return;
    }
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
    if close_popover(window) {
        return;
    }
    if let Some(dialog) = window.visible_dialog() {
        dialog.close();
        return;
    }
    if let Some(ui) = ui
        && !ui.close_now_playing()
    {
        ui.nav().pop();
    }
}
