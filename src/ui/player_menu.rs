//! The player's OSD menus (volume, audio, subtitles, quality, picture,
//! shaders, sleep), drawn inside the window over the video. A GtkPopover is a
//! surface of its own: under gamescope with vsync off (Game Mode) a second
//! surface drops the video to 10–20 fps while a menu is open.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

/// Marks the menu's frame; controller focus moves stay inside it
/// (`gamepad::move_focus`).
pub(super) const CSS_CLASS: &str = "player-menu";
const MAIN_PAGE: &str = "main";
/// Gap above the opening button, and the least distance to the edges.
const GAP: i32 = 6;
const EDGE: i32 = 12;

/// How a menu closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Closed {
    /// An entry was picked.
    Chosen,
    /// B / Escape, a click outside, or another controller button.
    Dismissed,
}

type ClosedCallback = Rc<dyn Fn(Closed)>;

pub(super) struct MenuPanel {
    /// Transparent, over the whole player while open: a click outside the
    /// menu lands here and closes it, as a popover's would.
    scrim: gtk::Box,
    frame: gtk::Box,
    stack: gtk::Stack,
    opener: RefCell<Option<gtk::Widget>>,
    on_closed: RefCell<Option<ClosedCallback>>,
}

impl MenuPanel {
    /// Adds the scrim and the (hidden) menu on top of `overlay`'s layers.
    pub(super) fn new(overlay: &gtk::Overlay) -> Rc<Self> {
        let scrim = gtk::Box::builder().visible(false).build();
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::SlideLeftRight)
            .vhomogeneous(false)
            .hhomogeneous(false)
            .interpolate_size(true)
            .build();
        // Never focusable itself, or it would hold the controller's focus
        // (see the 1.1.0 queue bug); its rows still take focus. Not
        // `can_focus(false)`: in GTK4 that also bars every row inside.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .focusable(false)
            .child(&stack)
            .build();
        let frame = gtk::Box::builder()
            .css_classes([CSS_CLASS])
            .visible(false)
            .build();
        frame.append(&scroller);
        overlay.add_overlay(&scrim);
        overlay.add_overlay(&frame);
        let panel = Rc::new(MenuPanel {
            scrim,
            frame,
            stack,
            opener: RefCell::new(None),
            on_closed: RefCell::new(None),
        });

        let click = gtk::GestureClick::new();
        click.set_button(0);
        let weak = Rc::downgrade(&panel);
        click.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(panel) = weak.upgrade() {
                panel.close(Closed::Dismissed);
            }
        });
        panel.scrim.add_controller(click);

        let weak = Rc::downgrade(&panel);
        overlay.connect_get_child_position(move |overlay, child| {
            let panel = weak.upgrade()?;
            if child != panel.frame.upcast_ref::<gtk::Widget>() {
                return None;
            }
            panel.place(overlay)
        });
        panel
    }

    /// Centred above the opening button, inside the player; scrolls when
    /// there's less room above the button than the menu needs.
    fn place(&self, overlay: &gtk::Overlay) -> Option<gdk::Rectangle> {
        let opener = self.opener.borrow().clone()?;
        let bounds = opener.compute_bounds(overlay)?;
        let (overlay_width, overlay_height) = (overlay.width(), overlay.height());
        let (_, natural_width, _, _) = self.frame.measure(gtk::Orientation::Horizontal, -1);
        let width = natural_width.min(overlay_width - 2 * EDGE).max(1);
        let (min_height, natural_height, _, _) =
            self.frame.measure(gtk::Orientation::Vertical, width);
        let top = bounds.y() as i32;
        let height = natural_height.min(top - GAP - EDGE).max(min_height);
        let centre = (bounds.x() + bounds.width() / 2.0) as i32;
        let x = (centre - width / 2).clamp(EDGE, (overlay_width - width - EDGE).max(EDGE));
        let y = (top - GAP - height).clamp(0, (overlay_height - height).max(0));
        Some(gdk::Rectangle::new(x, y, width, height))
    }

    pub(super) fn is_open(&self) -> bool {
        self.frame.is_visible()
    }

    pub(super) fn connect_closed(&self, callback: impl Fn(Closed) + 'static) {
        self.on_closed.replace(Some(Rc::new(callback)));
    }

    /// Shows `main` above `opener`; `pages` are reached with [`show_page`].
    /// Focus goes to `main`'s checked entry, else its first.
    pub(super) fn open(
        &self,
        opener: &impl IsA<gtk::Widget>,
        main: &gtk::Box,
        pages: Vec<(String, gtk::Widget)>,
    ) {
        self.close_quietly();
        self.stack.add_named(main, Some(MAIN_PAGE));
        for (name, page) in &pages {
            self.stack.add_named(page, Some(name));
        }
        self.stack
            .set_visible_child_full(MAIN_PAGE, gtk::StackTransitionType::None);
        self.opener.replace(Some(opener.clone().upcast()));
        self.scrim.set_visible(true);
        self.frame.set_visible(true);
        focus_when_mapped(main);
    }

    /// One of the pages given to [`open`], focused on its checked entry.
    pub(super) fn show_page(&self, name: &str) {
        self.stack.set_visible_child_name(name);
        if let Some(page) = self.stack.visible_child().and_downcast::<gtk::Box>() {
            focus_when_mapped(&page);
        }
    }

    pub(super) fn show_main(&self) {
        self.stack.set_visible_child_name(MAIN_PAGE);
        if let Some(page) = self.stack.visible_child().and_downcast::<gtk::Box>() {
            focus_when_mapped(&page);
        }
    }

    /// B / Escape: from a sub-page back to the main one, else closes.
    pub(super) fn back(&self) {
        if self.stack.visible_child_name().as_deref() == Some(MAIN_PAGE) {
            self.close(Closed::Dismissed);
        } else {
            self.show_main();
        }
    }

    /// Closes the menu, focus back on its button, and tells `connect_closed`.
    pub(super) fn close(&self, how: Closed) {
        if !self.is_open() {
            return;
        }
        let opener = self.close_quietly();
        if let Some(opener) = opener.filter(|o| o.is_mapped()) {
            opener.grab_focus();
        }
        let callback = self.on_closed.borrow().clone();
        if let Some(callback) = callback {
            callback(how);
        }
    }

    /// Hides and empties the menu (pages are rebuilt on each opening; the
    /// volume box is reused, so it must leave the stack). No callback.
    pub(super) fn close_quietly(&self) -> Option<gtk::Widget> {
        self.frame.set_visible(false);
        self.scrim.set_visible(false);
        while let Some(child) = self.stack.first_child() {
            self.stack.remove(&child);
        }
        self.opener.take()
    }
}

/// A menu model's items as rows, like a popover menu shows them: a leading
/// check on the item whose target is its action's state, a separator and
/// heading per labelled section. `chosen` runs after an item's action.
pub(super) fn model_page(
    model: &impl IsA<gio::MenuModel>,
    actions: &gio::SimpleActionGroup,
    chosen: Rc<dyn Fn()>,
) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    append_items(&page, model.upcast_ref(), actions, &chosen);
    page
}

fn append_items(
    page: &gtk::Box,
    model: &gio::MenuModel,
    actions: &gio::SimpleActionGroup,
    chosen: &Rc<dyn Fn()>,
) {
    let text = |index: i32, name: &str| {
        model
            .item_attribute_value(index, name, Some(glib::VariantTy::STRING))
            .and_then(|v| v.get::<String>())
    };
    for index in 0..model.n_items() {
        if let Some(section) = model.item_link(index, "section") {
            if page.first_child().is_some() {
                page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            }
            if let Some(title) = text(index, "label") {
                page.append(
                    &gtk::Label::builder()
                        .label(title)
                        .xalign(0.0)
                        .css_classes(["menu-heading", "caption-heading", "dim-label"])
                        .build(),
                );
            }
            append_items(page, &section, actions, chosen);
            continue;
        }
        let Some(action) = text(index, "action") else {
            continue;
        };
        let name = action.strip_prefix("player.").unwrap_or(&action).to_owned();
        let target = model.item_attribute_value(index, "target", None);
        let checked = match (&target, actions.action_state(&name)) {
            (Some(target), Some(state)) => *target == state,
            (None, Some(state)) => state.get::<bool>().unwrap_or(false),
            _ => false,
        };
        let mark = check_icon();
        mark.set_opacity(if checked { 1.0 } else { 0.0 });
        let row = menu_row(
            &text(index, "label").unwrap_or_default(),
            Some(&mark),
            None,
            None,
        );
        row.set_sensitive(actions.is_action_enabled(&name));
        let (actions, chosen) = (actions.clone(), chosen.clone());
        row.connect_clicked(move |_| {
            actions.activate_action(&name, target.as_ref());
            chosen();
        });
        page.append(&row);
    }
}

/// A menu entry: optional leading icon, label, then an optional mark and
/// trailing icon.
pub(super) fn menu_row(
    label: &str,
    leading: Option<&gtk::Image>,
    mark: Option<&gtk::Image>,
    trailing: Option<&str>,
) -> gtk::Button {
    let line = gtk::Box::builder().spacing(12).build();
    if let Some(leading) = leading {
        line.append(leading);
    }
    line.append(
        &gtk::Label::builder()
            .label(label)
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    if let Some(mark) = mark {
        line.append(mark);
    }
    if let Some(icon) = trailing {
        line.append(&gtk::Image::from_icon_name(icon));
    }
    gtk::Button::builder()
        .child(&line)
        .css_classes(["flat", "menu-row"])
        .build()
}

/// A check mark; hidden by opacity so rows keep their alignment.
pub(super) fn check_icon() -> gtk::Image {
    gtk::Image::from_icon_name(crate::ui::icons::WATCHED)
}

/// [`focus_checked`] once `page` is on screen: a widget that isn't mapped
/// yet refuses focus, and the controller's cursor would stay on the OSD
/// button behind the menu (Left/Right then moved along the bar, A reopened
/// menus, the Deck log showed menus reopening instead of sub-pages).
fn focus_when_mapped(page: &gtk::Box) {
    if page.is_mapped() {
        focus_checked(page);
        return;
    }
    let handler = std::rc::Rc::new(std::cell::Cell::new(None::<glib::SignalHandlerId>));
    let id = page.connect_map({
        let handler = handler.clone();
        move |page| {
            if let Some(id) = handler.take() {
                page.disconnect(id);
            }
            focus_checked(page);
        }
    });
    handler.set(Some(id));
}

/// Focuses the entry of `page` whose check shows, else its first entry.
pub(super) fn focus_checked(page: &gtk::Box) {
    let mut child = page.first_child();
    let mut first = None;
    while let Some(widget) = child {
        child = widget.next_sibling();
        let Some(row) = widget.downcast_ref::<gtk::Button>() else {
            continue;
        };
        // Skip the back row at the top.
        if row.has_css_class("heading") {
            continue;
        }
        first.get_or_insert_with(|| row.clone());
        let checked = row
            .child()
            .and_then(|line| line.first_child())
            .and_downcast::<gtk::Image>()
            .is_some_and(|mark| mark.opacity() > 0.5);
        if checked {
            row.grab_focus();
            return;
        }
    }
    if let Some(row) = first {
        row.grab_focus();
    }
}
