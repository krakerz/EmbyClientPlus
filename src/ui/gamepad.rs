//! Turns controller presses into UI actions: focus movement and
//! activation while browsing, playback control on the player page.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;

use super::{Ui, library, open_card_menu, preferences};
use crate::controller::{self, Action, Context, Pad};

/// Whether controller input should reach `window` now: not while another
/// app or game is in front, which gets the same presses. A window that has
/// never reported itself active keeps taking them, in case a compositor
/// (gamescope) never says.
pub fn input_gate(window: &impl IsA<gtk::Window>) -> impl Fn() -> bool + 'static {
    let seen_active = Rc::new(Cell::new(window.is_active()));
    window.connect_is_active_notify({
        let seen_active = seen_active.clone();
        move |window| {
            if window.is_active() {
                seen_active.set(true);
            }
        }
    });
    let window = window.as_ref().downgrade();
    move || {
        window
            .upgrade()
            .is_some_and(|window| window.is_active() || !seen_active.get())
    }
}

/// Handles one press for `window`; `ui` is the logged-in UI, if any (the
/// login form only needs focus movement and activation). `repeat`: a held
/// direction repeating rather than a fresh press.
pub fn handle(window: &adw::ApplicationWindow, ui: Option<&Ui>, pad: Pad, repeat: bool) {
    // Controller use means focus rings matter, like keyboard use.
    window.set_focus_visible(true);
    enter_pad_mode(window, ui.is_none_or(|ui| !ui.player_visible()));
    end_tab_switch();

    // The right stick scrolled the cursor out of view: a direction or A
    // first brings it onto something visible, rather than jumping back.
    if ui.is_none_or(|ui| !ui.player_visible())
        && matches!(
            controller::action_for(pad, Context::Browse),
            Some(Action::Up | Action::Down | Action::Left | Action::Right | Action::Activate)
        )
        && super::stick_scroll::refocus_if_scrolled_away(window)
    {
        return;
    }

    // The music panel: its own buttons first, then normal navigation.
    if let Some(ui) = ui
        && !ui.player_visible()
        && window.visible_dialog().is_none()
        && let Some(panel) = ui.music_panel()
        && panel.is_open()
    {
        if music_panel_press(window, ui, &panel, pad) {
            return;
        }
        // Home, Search, Preferences...: leave the panel, then do it.
        if !matches!(
            controller::action_for(pad, Context::Browse),
            Some(
                Action::Up
                    | Action::Down
                    | Action::Left
                    | Action::Right
                    | Action::Activate
                    | Action::Back
                    | Action::ContextMenu
            )
        ) {
            panel.close();
        } else {
            return browse_navigation(window, ui, pad);
        }
    }

    // A notification the cursor was taken to with LT/RT.
    if let Some(ui) = ui
        && !ui.player_visible()
        && window.visible_dialog().is_none()
        && let Some(focus) = gtk::prelude::GtkWindowExt::focus(window)
        && let Some(toast) = toast_of(&focus)
        && toast_press(ui, &toast, &focus, pad)
    {
        return;
    }

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
        && player_press(window, ui.player_page(), pad, repeat)
    {
        return;
    }
    let Some(action) = controller::action_for(pad, Context::Browse) else {
        return;
    };
    // An item menu is open: buttons that don't move through it close it,
    // rather than acting on the page behind.
    if !matches!(
        action,
        Action::Up | Action::Down | Action::Left | Action::Right | Action::Activate | Action::Back
    ) && close_popover(window)
    {
        return;
    }
    // Neither does anything behind an open dialog.
    let page_free = window.visible_dialog().is_none();
    match action {
        Action::Up => move_focus(window, gtk::DirectionType::Up),
        Action::Down => move_focus(window, gtk::DirectionType::Down),
        Action::Left => horizontal_in(window, false),
        Action::Right => horizontal_in(window, true),
        Action::Activate => activate_focus(window),
        Action::Back => back(window, ui),
        Action::Home if page_free => {
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
            let forward = action == Action::NextTab;
            // A dialog with tabs (Preferences) takes LB/RB over the page.
            if let Some(dialog) = window.visible_dialog() {
                if let Some(stack) = find_view_stack(dialog.upcast_ref()) {
                    switch_tab(&stack, || super::library_page::step_stack(&stack, forward));
                }
            } else if let Some(ui) = ui {
                // Library-style tabs: the cursor follows to the new tab.
                // (Seasons and episodes place it themselves.)
                let stack = ui
                    .nav()
                    .visible_page()
                    .filter(|page| super::tab_label(page) == "Tabs")
                    .and_then(|page| find_view_stack(page.upcast_ref()));
                match stack {
                    Some(stack) => switch_tab(&stack, || ui.step_tabs(forward)),
                    None => ui.step_tabs(forward),
                }
            }
        }
        Action::Search if page_free => {
            if let Some(ui) = ui {
                ui.start_search();
                if crate::osk::available() {
                    // The field slides in (a revealer): its rect is only
                    // right once that's done.
                    let window = window.downgrade();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(300),
                        move || {
                            if let Some(window) = window.upgrade()
                                && let Some(text) = gtk::prelude::GtkWindowExt::focus(&window)
                                    .and_downcast::<gtk::Text>()
                            {
                                open_keyboard(&text);
                            }
                        },
                    );
                }
            }
        }
        Action::Preferences if ui.is_some() && page_free => {
            preferences::show(window);
        }
        // LT/RT step through the page, a notification, the music panel.
        Action::OpenMusic if page_free => {
            if let Some(ui) = ui
                && !focus_toast(ui)
                && let Some(panel) = ui.music_panel()
            {
                panel.open();
            }
        }
        Action::JumpLetters if page_free => {
            if let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) {
                library::jump_letters(&focus);
            }
        }
        _ => {}
    }
}

/// A press while `page` (the player) is on screen. False when it's for
/// moving through an open menu, which the caller does as while browsing.
pub fn player_press(
    window: &adw::ApplicationWindow,
    page: &super::player_page::PlayerPage,
    pad: Pad,
    repeat: bool,
) -> bool {
    // Start: the OSD's buttons, driven like a page (directions move, A
    // presses or opens, B or Start leave; idle, or a finished action,
    // leaves too). Menus opened there work as below. Every press restarts
    // the idle countdown.
    if page.controls_mode() {
        page.touch_controls();
    }
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
        return true;
    }
    if !page.menu_open() {
        if let Some(action) = controller::action_for(pad, Context::Player) {
            page.controller_action(action, repeat);
        }
        return true;
    }
    // A player menu is open: the controller stays in the player. It moves
    // through the menu, picks or closes it; any other button (like the
    // one that opened it) just closes it, and never leaves the player.
    match controller::action_for(pad, Context::Browse) {
        Some(Action::Up | Action::Down | Action::Left | Action::Right | Action::Activate) => false,
        // B steps back a page, or closes without choosing: controls mode
        // keeps its button.
        Some(Action::Back) => {
            page.menu_back();
            true
        }
        Some(other) => {
            tracing::info!("player: menu closed by controller {other:?}");
            page.close_menu();
            true
        }
        // Not a mapped button (stick drift, extra buttons of a virtual pad):
        // nothing to do, and it mustn't close the menu.
        None => true,
    }
}

const PAD_MODE_CLASS: &str = "pad-mode";

/// The controller is driving: the window gets `pad-mode` (CSS drops hover
/// tints under a resting pointer) and, outside the player, hides the
/// pointer. Real pointer movement ends it (see `watch_pointer`).
fn enter_pad_mode(window: &adw::ApplicationWindow, hide_pointer: bool) {
    watch_pointer(window.upcast_ref());
    if let Some(popover) = gtk::prelude::GtkWindowExt::focus(window)
        .and_then(|f| f.ancestor(gtk::Popover::static_type()))
    {
        watch_pointer(&popover);
    }
    window.add_css_class(PAD_MODE_CLASS);
    // The window's own cursor only: the player page sets its own.
    if hide_pointer {
        window.set_cursor_from_name(Some("none"));
    }
}

fn leave_pad_mode(window: &gtk::Window) {
    if window.has_css_class(PAD_MODE_CLASS) {
        window.remove_css_class(PAD_MODE_CLASS);
        window.set_cursor(None::<&gtk::gdk::Cursor>);
    }
}

const POINTER_WATCH_KEY: &str = "embyclientplus-pointer-watch";

/// Ends pad mode when the pointer really moves over `widget` (the window,
/// or a popover: those have their own surface and don't pass events on).
/// GTK also sends motion when a widget appears under a resting pointer, so
/// the first event only records a position and small jitters are ignored.
fn watch_pointer(widget: &gtk::Widget) {
    // SAFETY: this key is only ever stored and read as `()`.
    if unsafe { widget.data::<()>(POINTER_WATCH_KEY) }.is_some() {
        return;
    }
    // SAFETY: see above.
    unsafe { widget.set_data(POINTER_WATCH_KEY, ()) };
    let last: Cell<Option<(f64, f64)>> = Cell::new(None);
    let moved = move |controller: &gtk::EventControllerMotion, x: f64, y: f64| {
        let previous = last.replace(Some((x, y)));
        let real = previous.is_some_and(|(px, py)| (x - px).abs() > 2.0 || (y - py).abs() > 2.0);
        if real
            && let Some(window) = controller
                .widget()
                .and_then(|w| w.root())
                .and_downcast::<gtk::Window>()
        {
            leave_pad_mode(&window);
        }
    };
    let motion = gtk::EventControllerMotion::new();
    motion.set_propagation_phase(gtk::PropagationPhase::Capture);
    let moved = Rc::new(moved);
    motion.connect_motion({
        let moved = moved.clone();
        move |controller, x, y| moved(controller, x, y)
    });
    motion.connect_enter(move |controller, x, y| moved(controller, x, y));
    widget.add_controller(motion);
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
        // A popover that took the focus (opened by mouse or pad) needs its
        // own pointer watch.
        if let Some(popover) = focus
            .as_ref()
            .and_then(|f| f.ancestor(gtk::Popover::static_type()))
        {
            watch_pointer(&popover);
        }
        keyboard_follow_focus(focus.as_ref());
        if TAB_SWITCH.get() == 0 {
            focus_changed(window);
        }
    });
}

/// The keyboard we opened (Game Mode), so we only close one of ours.
struct Keyboard {
    /// The whole field (an Entry and its inner Text, an EntryRow...).
    field: glib::WeakRef<gtk::Widget>,
    text: glib::WeakRef<gtk::Text>,
    on_activate: glib::SignalHandlerId,
}

thread_local! {
    static KEYBOARD: RefCell<Option<Keyboard>> = const { RefCell::new(None) };
}

/// Opens Steam's keyboard for the field holding `text`.
fn open_keyboard(text: &gtk::Text) {
    forget_keyboard();
    // The outermost editable around the text is the whole field.
    let mut field: gtk::Widget = text.clone().upcast();
    while let Some(parent) = field.parent().filter(|p| p.is::<gtk::Editable>()) {
        field = parent;
    }
    crate::osk::open(field_rect(&field));
    // Enter on Steam's keyboard submits and dismisses it there.
    let on_activate = text.connect_activate(|_| {
        forget_keyboard();
    });
    KEYBOARD.with(|k| {
        k.replace(Some(Keyboard {
            field: field.downgrade(),
            text: text.downgrade(),
            on_activate,
        }))
    });
}

/// Drops the record of an open keyboard without telling Steam (it's gone
/// already, or about to be reopened). True if there was one.
fn forget_keyboard() -> bool {
    let Some(keyboard) = KEYBOARD.with(|k| k.take()) else {
        return false;
    };
    if let Some(text) = keyboard.text.upgrade() {
        text.disconnect(keyboard.on_activate);
    }
    true
}

/// Closes the keyboard, if it's one we opened.
fn close_keyboard() {
    if forget_keyboard() {
        crate::osk::close();
    }
}

/// Focus moved: the keyboard goes unless it stayed in the same field (a
/// suggestion popover inside the Entry keeps it).
fn keyboard_follow_focus(focus: Option<&gtk::Widget>) {
    let stays = KEYBOARD.with(|k| {
        k.borrow()
            .as_ref()
            .map(|k| match (k.field.upgrade(), focus) {
                (Some(field), Some(f)) => *f == field || f.is_ancestor(&field),
                _ => false,
            })
    });
    if stays == Some(false) {
        close_keyboard();
    }
}

/// The field's rect in physical pixels, relative to the window, as SDL
/// reports it to Steam.
fn field_rect(field: &gtk::Widget) -> Option<[i32; 4]> {
    let root = field.root()?;
    let bounds = field.compute_bounds(&root)?;
    let scale = f64::from(field.native()?.surface()?.scale_factor());
    if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return None;
    }
    let px = |v: f32| (f64::from(v) * scale).round() as i32;
    Some([
        px(bounds.x()),
        px(bounds.y()),
        px(bounds.width()),
        px(bounds.height()),
    ])
}

/// Remembers where the cursor now is, or recovers it when it's lost.
fn focus_changed(window: &adw::ApplicationWindow) {
    let focus = gtk::prelude::GtkWindowExt::focus(window);
    // The focused widget went away (a list or page rebuilt after an
    // action, a page opened): put the cursor back where it was, or on
    // the page's first item.
    let Some(focus) = focus.filter(|f| !is_container(f)) else {
        let container = gtk::prelude::GtkWindowExt::focus(window);
        recover_focus(window, container);
        return;
    };
    // Not a spot to come back to: when the notification goes, the cursor
    // returns to the page.
    if toast_of(&focus).is_some() {
        return;
    }
    let anchor: gtk::Widget = focus
        .ancestor(adw::NavigationPage::static_type())
        .unwrap_or_else(|| window.clone().upcast());
    let Some(spot) = Spot::of(&anchor, &focus) else {
        return;
    };
    if let Some(page) = anchor.downcast_ref::<adw::NavigationPage>() {
        // SAFETY: only ever stored and read as `Spot`.
        unsafe { page.set_data(LAST_FOCUS_KEY, spot.clone()) };
    }
    // Each tab (library tabs, Preferences pages) keeps its own spot too.
    if let Some(tab) = tab_root(&focus)
        && let Some(tab_spot) = Spot::of(&tab, &focus)
    {
        // SAFETY: only ever stored and read as `Spot`.
        unsafe { tab.set_data(LAST_FOCUS_KEY, tab_spot) };
    }
    LAST_SPOT.with(|last| last.replace(Some(spot)));
}

thread_local! {
    /// Where the cursor last was, anywhere in the window.
    static LAST_SPOT: RefCell<Option<Spot>> = const { RefCell::new(None) };
    /// The tab switch still placing the cursor (0: none); see [`switch_tab`].
    static TAB_SWITCH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    static NEXT_TAB_SWITCH: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
}

/// Where the cursor was: the widget, and its position (child indexes)
/// under its page — or under the window, outside pages (music panel,
/// dialogs). When a rebuild replaces the widget, the same position in the
/// new content is the same item (row 5 of the list is still row 5).
#[derive(Clone)]
struct Spot {
    widget: glib::WeakRef<gtk::Widget>,
    anchor: glib::WeakRef<gtk::Widget>,
    path: Vec<usize>,
}

impl Spot {
    fn of(anchor: &gtk::Widget, widget: &gtk::Widget) -> Option<Spot> {
        let mut path = Vec::new();
        let mut current = widget.clone();
        while &current != anchor {
            let mut index = 0;
            let mut sibling = current.prev_sibling();
            while let Some(previous) = sibling {
                index += 1;
                sibling = previous.prev_sibling();
            }
            path.push(index);
            current = current.parent()?;
        }
        path.reverse();
        Some(Spot {
            widget: widget.downgrade(),
            anchor: anchor.downgrade(),
            path,
        })
    }

    /// Puts the cursor back: on the widget if it's still there, else at
    /// its position. `exact`: only if that whole position exists again
    /// (content still loading otherwise); else the nearest one.
    fn restore(&self, exact: bool) -> bool {
        if let Some(widget) = self.widget.upgrade()
            && usable(&widget)
            && widget.grab_focus()
        {
            return true;
        }
        let Some(anchor) = self.anchor.upgrade().filter(|a| a.is_mapped()) else {
            return false;
        };
        let mut current = anchor;
        for &index in &self.path {
            let mut child = current.first_child();
            let mut position = 0;
            let mut last = None;
            while let Some(c) = child {
                if position == index {
                    last = Some(c.clone());
                    break;
                }
                position += 1;
                child = c.next_sibling();
                last = Some(c);
            }
            let Some(next) = last else {
                break;
            };
            if position != index && exact {
                return false;
            }
            current = next;
        }
        if usable(&current) && current.grab_focus() {
            return true;
        }
        if exact && !current.is_mapped() {
            return false;
        }
        current.is_mapped() && focus_first_inside(&current)
    }
}

/// The tab `widget` is in: the visible child of its nearest tab stack
/// (where [`place_in_tab`] looks for the tab's spot).
fn tab_root(widget: &gtk::Widget) -> Option<gtk::Widget> {
    let stack = widget
        .ancestor(adw::ViewStack::static_type())
        .and_downcast::<adw::ViewStack>()?;
    stack.visible_child().filter(|tab| widget.is_ancestor(tab))
}

/// Switches `stack`'s tab with `step`, then puts the cursor on the new
/// tab's own spot (or its first item) as soon as its content is laid out.
/// Until then the cursor is in limbo: anything it lands on meanwhile is
/// neither remembered nor recovered from, or it shows as a sideways jump.
fn switch_tab(stack: &adw::ViewStack, step: impl FnOnce()) {
    let id = NEXT_TAB_SWITCH.get();
    NEXT_TAB_SWITCH.set(id.wrapping_add(1).max(1));
    TAB_SWITCH.set(id);
    step();
    let stack = stack.downgrade();
    let started = std::time::Instant::now();
    glib::timeout_add_local(TAB_RETRY, move || {
        // A newer press took over.
        if TAB_SWITCH.get() != id {
            return glib::ControlFlow::Break;
        }
        let stack = stack.upgrade().filter(|s| s.is_mapped());
        let placed = stack.as_ref().is_some_and(place_in_tab);
        if !placed && stack.is_some() && started.elapsed() < TAB_PATIENCE {
            return glib::ControlFlow::Continue;
        }
        TAB_SWITCH.set(0);
        if let Some(window) = stack
            .and_then(|s| s.root())
            .and_downcast::<adw::ApplicationWindow>()
        {
            focus_changed(&window);
        }
        glib::ControlFlow::Break
    });
}

/// Ends a pending [`switch_tab`]: a new press acts on the cursor as it is.
fn end_tab_switch() {
    TAB_SWITCH.set(0);
}

const TAB_RETRY: std::time::Duration = std::time::Duration::from_millis(30);
const TAB_PATIENCE: std::time::Duration = std::time::Duration::from_secs(3);

/// The cursor goes to the visible tab's own last spot, or its first item.
/// True once it's on a real item there.
fn place_in_tab(stack: &adw::ViewStack) -> bool {
    let (Some(tab), Some(root)) = (stack.visible_child(), stack.root()) else {
        return false;
    };
    let focus = root.focus();
    // The stack put it back on the tab's last item itself.
    if focus
        .as_ref()
        .is_some_and(|f| usable(f) && f.is_ancestor(&tab))
    {
        return true;
    }
    // A scroller took the cursor before its content was in: start over, or
    // moving on from it never reaches the content.
    if focus.is_some_and(|f| !usable(&f)) {
        root.set_focus(None::<&gtk::Widget>);
    }
    // SAFETY: see `track_focus`; cloned out immediately.
    let spot = unsafe { tab.data::<Spot>(LAST_FOCUS_KEY).map(|s| s.as_ref().clone()) };
    let placed = match spot {
        Some(spot) => spot.restore(false),
        None => focus_first_inside(&tab),
    };
    placed
        && root
            .focus()
            .is_some_and(|f| usable(&f) && f.is_ancestor(&tab))
}

/// Where the cursor is inside `anchor`, taken before rebuilding that part
/// of the window, so it can go back to the same position afterwards.
pub(super) struct CursorMark(Spot);

pub(super) fn mark_cursor(anchor: &impl IsA<gtk::Widget>) -> Option<CursorMark> {
    let anchor = anchor.as_ref();
    let focus = anchor.root()?.focus()?;
    if !focus.is_ancestor(anchor) {
        return None;
    }
    Spot::of(anchor, &focus).map(CursorMark)
}

/// [`mark_cursor`] for a whole page, falling back to the spot it last had
/// the cursor: a page coming back into view may rebuild before its own
/// focus is back.
pub(super) fn mark_page_cursor(page: &adw::NavigationPage) -> Option<CursorMark> {
    mark_cursor(page).or_else(|| {
        // SAFETY: see `track_focus`; cloned out immediately.
        let spot = unsafe {
            page.data::<Spot>(LAST_FOCUS_KEY)
                .map(|s| s.as_ref().clone())
        };
        spot.map(CursorMark)
    })
}

impl CursorMark {
    /// Back to the marked position (or the nearest one that exists).
    pub(super) fn restore(&self) -> bool {
        self.0.restore(false)
    }
}

/// A widget the cursor can sit on.
pub(super) fn usable(widget: &gtk::Widget) -> bool {
    widget.is_mapped() && widget.is_sensitive() && widget.is_focusable() && !is_container(widget)
}

/// The cursor is on nothing, or on a container: bring it back to its last
/// spot (once it exists again) or, on a newly opened page, to where the
/// page starts it. Retries for a moment while content loads.
fn recover_focus(window: &adw::ApplicationWindow, container: Option<gtk::Widget>) {
    let page = container
        .as_ref()
        .and_then(|c| c.ancestor(adw::NavigationPage::static_type()))
        .and_downcast::<adw::NavigationPage>();
    // SAFETY: see `track_focus`; cloned out immediately.
    let page_spot = page.as_ref().and_then(|page| unsafe {
        page.data::<Spot>(LAST_FOCUS_KEY)
            .map(|s| s.as_ref().clone())
    });
    let spot = match (&container, page_spot) {
        (Some(_), Some(spot)) => Some(spot),
        // A page seen for the first time starts where it wants to.
        (Some(_), None) if page.is_some() => None,
        _ => LAST_SPOT.with(|last| last.borrow().clone()),
    };
    if container.is_none() && spot.is_none() {
        return;
    }
    let window = window.downgrade();
    let container = container.map(|c| c.downgrade());
    let tries = std::cell::Cell::new(0);
    glib::timeout_add_local(CONTAINER_RETRY, move || {
        tries.set(tries.get() + 1);
        let Some(window) = window.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let focus = gtk::prelude::GtkWindowExt::focus(&window);
        let container = container.as_ref().and_then(|c| c.upgrade());
        // Something else took the cursor meanwhile (the user moved).
        let still_lost = match (&focus, &container) {
            (None, _) => true,
            (Some(focus), Some(container)) => focus == container,
            (Some(focus), None) => !usable(focus),
        };
        if !still_lost {
            return glib::ControlFlow::Break;
        }
        let last_try = tries.get() >= CONTAINER_TRIES;
        let placed = match &spot {
            Some(spot) => spot.restore(!last_try),
            None => false,
        };
        if placed {
            return glib::ControlFlow::Break;
        }
        if let Some(container) = &container
            && (spot.is_none() || last_try)
            && focus_start(container)
        {
            return glib::ControlFlow::Break;
        }
        if last_try {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

const CONTAINER_RETRY: std::time::Duration = std::time::Duration::from_millis(250);
/// About 3 s of retries for a page whose content is still loading.
const CONTAINER_TRIES: u32 = 12;

/// Widgets that can hold focus but do nothing with a press.
fn is_container(widget: &gtk::Widget) -> bool {
    widget.is::<gtk::WindowControls>()
        || widget.is::<gtk::ScrolledWindow>()
        || widget.is::<gtk::Viewport>()
        || widget.is::<gtk::ListBox>()
        || widget.is::<adw::NavigationPage>()
        || widget.is::<adw::ToolbarView>()
}

/// The first tab stack inside `widget` (e.g. Preferences' pages).
fn find_view_stack(widget: &gtk::Widget) -> Option<adw::ViewStack> {
    if let Some(stack) = widget.downcast_ref::<adw::ViewStack>() {
        return Some(stack.clone());
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        if let Some(found) = find_view_stack(&current) {
            return Some(found);
        }
        child = current.next_sibling();
    }
    None
}

/// Puts the cursor where `container`'s page wants it to start (its focus
/// hook), else on the first item inside.
fn focus_start(container: &gtk::Widget) -> bool {
    let hook = container
        .ancestor(adw::NavigationPage::static_type())
        .and_downcast::<adw::NavigationPage>()
        .and_then(|page| super::focus_hook(&page));
    match hook {
        Some(hook) => hook(),
        None => focus_first_inside(container),
    }
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
    let row = widget
        .downcast_ref::<gtk::ListBoxRow>()
        .map(|row| format!(" #{}", row.index()))
        .or_else(|| {
            widget
                .downcast_ref::<gtk::Button>()
                .and_then(|b| b.label())
                .map(|label| format!(" '{label}'"))
        })
        .unwrap_or_default();
    let mut chain = vec![format!(
        "{}{row}{}",
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
    let page = widget
        .ancestor(adw::NavigationPage::static_type())
        .and_downcast::<adw::NavigationPage>()
        .map(|page| page.title().to_string())
        .unwrap_or_default();
    format!("[{page}] {}", chain.join(" < "))
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
    focus_page(ui)
}

/// Puts the cursor back on the visible page: its last focused widget,
/// else where the page wants it to start, else its first item.
fn focus_page(ui: &Ui) -> bool {
    let Some(page) = ui.nav().visible_page() else {
        return false;
    };
    // SAFETY: see `track_focus`; cloned out immediately.
    let spot = unsafe {
        page.data::<Spot>(LAST_FOCUS_KEY)
            .map(|s| s.as_ref().clone())
    };
    if spot.is_some_and(|spot| spot.restore(false)) {
        return true;
    }
    if let Some(hook) = super::focus_hook(&page)
        && hook()
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
/// it (the window, an open popover, or a dialog), or inside the player's
/// open menu.
fn move_focus(window: &adw::ApplicationWindow, direction: gtk::DirectionType) {
    let focus = gtk::prelude::GtkWindowExt::focus(window);
    // A popover hangs off a widget inside the grid (the card), so the grid's
    // own rules would walk out of it: inside one, only its entries count.
    // The player's in-window menu is moved through the same way.
    let menu = focus.as_ref().and_then(player_menu_of);
    let in_popover = menu.is_some()
        || focus
            .as_ref()
            .is_some_and(|f| f.ancestor(gtk::Popover::static_type()).is_some());
    if !in_popover {
        if let Some(focus) = &focus
            && (library::move_in_grid(focus, direction) || library::move_in_strip(focus, direction))
        {
            return;
        }
        // Up from a grid's top row: its bar (filter, sort), which sits
        // centred and so isn't straight above most cards for GTK's own
        // focus move.
        if direction == gtk::DirectionType::Up
            && let Some(focus) = &focus
            && let Some(bar) = library::grid_header(focus)
            && let Some(target) =
                nearest_in_direction(&bar, focus, direction).or_else(|| first_focusable(&bar))
        {
            target.grab_focus();
            return;
        }
    }
    let mut scope: gtk::Widget = menu.unwrap_or_else(|| {
        focus
            .as_ref()
            .and_then(|widget| widget.native())
            .map(|native| native.upcast())
            .unwrap_or_else(|| window.clone().upcast())
    });
    // An open dialog is drawn inside the window: never step out of it onto
    // the page behind.
    if scope == *window.upcast_ref::<gtk::Widget>()
        && let Some(dialog) = window.visible_dialog()
    {
        scope = dialog.upcast();
    }
    let Some(focus) = focus else {
        scope.child_focus(gtk::DirectionType::TabForward);
        return;
    };
    let moved = scope.child_focus(direction);
    // Inside a popover, stop at its ends: no wrapping round, no stepping to
    // another row on Left/Right.
    if in_popover
        && let Some(now) = gtk::prelude::GtkWindowExt::focus(window)
        && now != focus
        && wanders(&scope, &focus, &now, direction)
    {
        focus.grab_focus();
        return;
    }
    // Some widgets keep the focus at their edge (a list's first row going
    // up, for one): then take the nearest item on screen that way.
    if gtk::prelude::GtkWindowExt::focus(window).as_ref() == Some(&focus) {
        let target = nearest_in_direction(&scope, &focus, direction);
        if std::env::var_os("EMBYCLIENTPLUS_DEBUG_FOCUS").is_some() {
            tracing::info!(
                "focus stuck ({direction:?}, child_focus {moved}); nearest: {}",
                describe(target.as_ref())
            );
        }
        if let Some(target) = target {
            target.grab_focus();
        }
    }
}

/// Whether focus moving from `from` to `to` went against `direction`, or
/// (Left/Right) onto another row.
/// The player's open menu holding `widget`, if any (`player_menu`).
fn player_menu_of(widget: &gtk::Widget) -> Option<gtk::Widget> {
    let mut current = Some(widget.clone());
    while let Some(w) = current {
        if w.has_css_class(super::player_menu::CSS_CLASS) {
            return Some(w);
        }
        current = w.parent();
    }
    None
}

fn wanders(
    scope: &gtk::Widget,
    from: &gtk::Widget,
    to: &gtk::Widget,
    direction: gtk::DirectionType,
) -> bool {
    let (Some(a), Some(b)) = (from.compute_bounds(scope), to.compute_bounds(scope)) else {
        return false;
    };
    let (ax, ay) = (a.x() + a.width() / 2.0, a.y() + a.height() / 2.0);
    let (bx, by) = (b.x() + b.width() / 2.0, b.y() + b.height() / 2.0);
    match direction {
        gtk::DirectionType::Up => by > ay + 1.0,
        gtk::DirectionType::Down => by < ay - 1.0,
        gtk::DirectionType::Left => bx > ax + 1.0 || (by - ay).abs() > a.height() / 2.0,
        gtk::DirectionType::Right => bx < ax - 1.0 || (by - ay).abs() > a.height() / 2.0,
        _ => false,
    }
}

/// The focusable item closest to `from` in `direction`, by on-screen
/// position within `scope`: mostly ahead of it, then least off to the side.
fn nearest_in_direction(
    scope: &gtk::Widget,
    from: &gtk::Widget,
    direction: gtk::DirectionType,
) -> Option<gtk::Widget> {
    let origin = from.compute_bounds(scope)?;
    let mut best: Option<(f32, gtk::Widget)> = None;
    let mut stack = vec![scope.clone()];
    while let Some(widget) = stack.pop() {
        if !widget.is_mapped() || !widget.is_sensitive() || widget.is_ancestor(from) {
            continue;
        }
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            stack.push(current);
        }
        if &widget == from || !widget.is_focusable() || is_container(&widget) {
            continue;
        }
        // Items' own parts (a card button's label) aren't targets.
        if from.is_ancestor(&widget) {
            continue;
        }
        let Some(rect) = widget.compute_bounds(scope) else {
            continue;
        };
        let Some(score) = direction_score(&origin, &rect, direction) else {
            continue;
        };
        if best.as_ref().is_none_or(|(b, _)| score < *b) {
            best = Some((score, widget));
        }
    }
    best.map(|(_, widget)| widget)
}

/// How far `to` is from `from` going `direction`; `None` when it isn't
/// that way at all. Sideways offset counts double, so the item straight
/// ahead wins over a closer one off to the side.
fn direction_score(
    from: &gtk::graphene::Rect,
    to: &gtk::graphene::Rect,
    direction: gtk::DirectionType,
) -> Option<f32> {
    let (fx0, fy0, fx1, fy1) = (
        from.x(),
        from.y(),
        from.x() + from.width(),
        from.y() + from.height(),
    );
    let (tx0, ty0, tx1, ty1) = (to.x(), to.y(), to.x() + to.width(), to.y() + to.height());
    // Gap between the spans on the other axis (0 when they overlap).
    let gap = |a0: f32, a1: f32, b0: f32, b1: f32| (b0 - a1).max(a0 - b1).max(0.0);
    const SLACK: f32 = 2.0;
    let (ahead, side) = match direction {
        gtk::DirectionType::Up => (fy0 - ty1, gap(fx0, fx1, tx0, tx1)),
        gtk::DirectionType::Down => (ty0 - fy1, gap(fx0, fx1, tx0, tx1)),
        gtk::DirectionType::Left => (fx0 - tx1, gap(fy0, fy1, ty0, ty1)),
        gtk::DirectionType::Right => (tx0 - fx1, gap(fy0, fy1, ty0, ty1)),
        _ => return None,
    };
    (ahead >= -SLACK).then_some(ahead.max(0.0) + side * 2.0)
}

/// A press while the music panel is open; false when it's not one of the
/// panel's own (it's then plain navigation, or leaves the panel).
fn music_panel_press(
    window: &adw::ApplicationWindow,
    ui: &Ui,
    panel: &super::music::MusicPanel,
    pad: Pad,
) -> bool {
    let focus = gtk::prelude::GtkWindowExt::focus(window);
    // The cursor belongs in the panel while it's open.
    if !focus
        .as_ref()
        .is_some_and(|f| f.is_mapped() && panel.contains(f) && !is_container(f))
    {
        panel.focus_default();
        return matches!(
            controller::action_for(pad, Context::Browse),
            Some(Action::Up | Action::Down | Action::Left | Action::Right)
        );
    }
    // A queue item picked up with X: ↑/↓ move it, A/B/X put it down.
    if panel.moving().is_some() {
        match (
            controller::action_for(pad, Context::Browse),
            controller::action_for(pad, Context::Music),
        ) {
            (Some(Action::Up), _) => panel.move_step(true),
            (Some(Action::Down), _) => panel.move_step(false),
            (Some(Action::Activate | Action::Back), _) | (_, Some(Action::MoveInQueue)) => {
                panel.end_move()
            }
            _ => {}
        }
        return true;
    }
    let Some(action) = controller::action_for(pad, Context::Music) else {
        return false;
    };
    if !panel.action(action, focus.as_ref()) {
        return false;
    }
    if action == Action::CloseMusic {
        focus_page(ui);
    }
    true
}

/// A press while the cursor is on a notification: ←/→ between its buttons,
/// A presses one, B goes back to the page and LT/RT on to the music panel.
/// False for anything else, which then acts on the page.
fn toast_press(ui: &Ui, toast: &gtk::Widget, focus: &gtk::Widget, pad: Pad) -> bool {
    match controller::action_for(pad, Context::Browse) {
        Some(direction @ (Action::Left | Action::Right)) => {
            let buttons = toast_buttons(toast);
            let next = buttons.iter().position(|b| b == focus).and_then(|at| {
                if direction == Action::Right {
                    buttons.get(at + 1)
                } else {
                    at.checked_sub(1).and_then(|at| buttons.get(at))
                }
            });
            if let Some(next) = next {
                next.grab_focus();
            }
        }
        Some(Action::Up | Action::Down) => {}
        Some(Action::Activate) => {
            // A pressed toast goes away; the cursor mustn't go with it.
            focus_page(ui);
            focus.activate();
        }
        Some(Action::Back) => {
            focus_page(ui);
        }
        Some(Action::OpenMusic) => {
            if !ui.music_panel().is_some_and(|panel| panel.open()) {
                focus_page(ui);
            }
        }
        _ => {
            focus_page(ui);
            return false;
        }
    }
    true
}

/// Puts the cursor on the notification showing, if any: its button, else
/// its close button.
fn focus_toast(ui: &Ui) -> bool {
    shown_toast(ui.toast_overlay())
        .and_then(|toast| toast_buttons(&toast).into_iter().next())
        .is_some_and(|button| button.grab_focus())
}

/// libadwaita doesn't expose a toast's widget: it's the overlay's child
/// styled `toast`. The newest comes last (an older one may be fading out).
fn shown_toast(overlay: &adw::ToastOverlay) -> Option<gtk::Widget> {
    let mut shown = None;
    let mut child = overlay.first_child();
    while let Some(current) = child {
        if current.css_name() == "toast" && current.is_mapped() {
            shown = Some(current.clone());
        }
        child = current.next_sibling();
    }
    shown
}

/// The notification `widget` is in, if any.
fn toast_of(widget: &gtk::Widget) -> Option<gtk::Widget> {
    let mut current = widget.parent();
    while let Some(parent) = current {
        if parent.css_name() == "toast" {
            return Some(parent);
        }
        current = parent.parent();
    }
    None
}

/// A toast's buttons, left to right: its own (Update, Restart), then close.
fn toast_buttons(toast: &gtk::Widget) -> Vec<gtk::Widget> {
    fn collect(widget: &gtk::Widget, buttons: &mut Vec<gtk::Widget>) {
        let mut child = widget.first_child();
        while let Some(current) = child {
            if current.is::<gtk::Button>() {
                if usable(&current) {
                    buttons.push(current.clone());
                }
            } else {
                collect(&current, buttons);
            }
            child = current.next_sibling();
        }
    }
    let mut buttons = Vec::new();
    collect(toast, &mut buttons);
    buttons
}

/// Directions, select, back and item menu, as while browsing.
fn browse_navigation(window: &adw::ApplicationWindow, ui: &Ui, pad: Pad) {
    match controller::action_for(pad, Context::Browse) {
        Some(Action::Up) => move_focus(window, gtk::DirectionType::Up),
        Some(Action::Down) => move_focus(window, gtk::DirectionType::Down),
        Some(Action::Left) => horizontal_in(window, false),
        Some(Action::Right) => horizontal_in(window, true),
        Some(Action::Activate) => activate_focus(window),
        Some(Action::Back) => back(window, Some(ui)),
        Some(Action::ContextMenu) => {
            if let Some(focus) = gtk::prelude::GtkWindowExt::focus(window) {
                open_card_menu(&focus);
            }
        }
        _ => {}
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
    // A, on a text field, brings up Steam's keyboard (Game Mode); Enter
    // there submits.
    if crate::osk::available()
        && let Some(text) = focus.downcast_ref::<gtk::Text>()
    {
        open_keyboard(text);
        return;
    }
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
    // The same for a drop-down (a grid's filter and sort): its list opens
    // but its choices don't take a press.
    let dropdown = focus
        .downcast_ref::<gtk::DropDown>()
        .cloned()
        .or_else(|| focus.ancestor(gtk::DropDown::static_type()).and_downcast());
    if let Some(dropdown) = dropdown {
        let count = dropdown.model().map_or(0, |m| m.n_items());
        if count > 0 {
            let selected = dropdown.selected();
            dropdown.set_selected(if selected == gtk::INVALID_LIST_POSITION {
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

/// Closes the innermost thing open: the A-Z strip, a popover, a dialog, then
/// a page.
fn back(window: &adw::ApplicationWindow, ui: Option<&Ui>) {
    close_keyboard();
    // In the A-Z strip: back to the list, not out of the page.
    if let Some(focus) = gtk::prelude::GtkWindowExt::focus(window)
        && library::leave_strip(&focus)
    {
        return;
    }
    if close_popover(window) {
        return;
    }
    if let Some(dialog) = window.visible_dialog() {
        dialog.close();
        return;
    }
    if let Some(ui) = ui {
        if ui.close_now_playing() {
            // Back where the cursor was before the panel opened.
            focus_page(ui);
        } else {
            ui.nav().pop();
        }
    }
}

/// The first focusable, visible widget inside `root`, in tree order.
fn first_focusable(root: &gtk::Widget) -> Option<gtk::Widget> {
    let mut stack = vec![root.clone()];
    while let Some(widget) = stack.pop() {
        if !widget.is_mapped() || !widget.is_sensitive() {
            continue;
        }
        if widget != *root && widget.is_focusable() && !is_container(&widget) {
            return Some(widget);
        }
        let mut children = Vec::new();
        let mut child = widget.first_child();
        while let Some(current) = child {
            child = current.next_sibling();
            children.push(current);
        }
        stack.extend(children.into_iter().rev());
    }
    None
}
