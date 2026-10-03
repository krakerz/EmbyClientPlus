//! The music player's UI: a mini player bar under every page while music
//! plays, opening into the music panel (Now Playing on the left, the queue
//! on the right). With a controller the panel has its own buttons (see
//! [`MusicPanel::action`]): LB/RB tracks, Y play/pause, X picks up a queue
//! item to move with ↑/↓, LT/RT back to the library.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

use super::{Ui, WeakUi, fixed_picture, format_timestamp, icons, images, placeholder_for};
use crate::emby::models::BaseItem;
use crate::music::{MusicPlayer, Repeat};
use crate::playback::TICKS_PER_SECOND;

const BAR_COVER: i32 = 48;
const SHEET_COVER: i32 = 260;
/// The mini player's width; the open panel matches it.
const BAR_WIDTH: i32 = 420;
const PANEL_WIDTH: i32 = BAR_WIDTH - 36;
const TICK: Duration = Duration::from_millis(500);

struct View {
    sheet: adw::BottomSheet,
    bar: gtk::Box,
    bar_cover: gtk::Picture,
    bar_title: gtk::Label,
    bar_artist: gtk::Label,
    bar_play: gtk::Button,
    bar_progress: gtk::ProgressBar,
    cover: gtk::Picture,
    title: gtk::Label,
    artist: gtk::Button,
    album: gtk::Button,
    seek: gtk::Scale,
    elapsed: gtk::Label,
    total: gtk::Label,
    play: gtk::Button,
    shuffle: gtk::ToggleButton,
    repeat: gtk::Button,
    volume: gtk::Scale,
    queue: gtk::ListBox,
    /// What the queue list shows (ids + current position), to skip
    /// rebuilding it when only the play state changed.
    queue_shown: RefCell<(Vec<String>, usize)>,
    /// The item the cover/links show.
    shown: RefCell<Option<String>>,
    /// Set while updating widgets from the player, so their signals don't
    /// echo back into it.
    syncing: Cell<bool>,
    /// The user is dragging the seek bar.
    seeking: Cell<bool>,
    /// The queue row being moved with the controller (its position).
    moving: Cell<Option<usize>>,
    /// The queue row to put the cursor on once the list is rebuilt.
    refocus: Cell<Option<usize>>,
}

/// The music panel, for the controller.
#[derive(Clone)]
pub struct MusicPanel {
    view: Rc<View>,
    music: crate::music::WeakMusicPlayer,
}

/// Fills `sheet` (around the navigation view) with the mini player and
/// Now Playing.
pub fn attach(ui: &Ui, music: &MusicPlayer, sheet: &adw::BottomSheet) -> MusicPanel {
    sheet.set_can_open(false);
    // A compact floating mini player at the bottom right (the controller
    // legend has the bottom left); the panel opens upward from it, just as
    // wide.
    sheet.set_full_width(false);
    sheet.set_align(1.0);

    // Mini player.
    let bar_cover = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .build();
    let bar_cover_frame = cover_frame(&bar_cover, BAR_COVER);
    let bar_title = label(&["heading"]);
    let bar_artist = label(&["dim-label", "caption"]);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .build();
    text.append(&bar_title);
    text.append(&bar_artist);
    let bar_previous = icon_button(icons::SKIP_BACK, "Previous");
    let bar_play = icon_button(icons::PAUSE, "Play/Pause");
    let bar_next = icon_button(icons::SKIP_FORWARD, "Next");
    let row = gtk::Box::builder()
        .spacing(12)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(12)
        .margin_end(12)
        .build();
    row.append(&bar_cover_frame);
    row.append(&text);
    row.append(&bar_previous);
    row.append(&bar_play);
    row.append(&bar_next);
    let bar_progress = gtk::ProgressBar::builder()
        .css_classes(["osd", "music-progress"])
        .build();
    let bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .width_request(BAR_WIDTH)
        .build();
    bar.append(&bar_progress);
    bar.append(&row);

    // Now Playing.
    let cover = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .build();
    let cover_box = cover_frame(&cover, SHEET_COVER);
    cover_box.set_halign(gtk::Align::Center);
    let title = gtk::Label::builder()
        .wrap(true)
        .justify(gtk::Justification::Center)
        .css_classes(["title-2"])
        .build();
    let artist = link_button();
    let album = link_button();
    let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
    seek.set_draw_value(false);
    let elapsed = label(&["dim-label", "numeric", "caption"]);
    let total = label(&["dim-label", "numeric", "caption"]);
    total.set_xalign(1.0);
    total.set_hexpand(true);
    let times = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    times.append(&elapsed);
    times.append(&total);
    let shuffle = gtk::ToggleButton::builder()
        .icon_name(icons::SHUFFLE)
        .tooltip_text("Shuffle")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    let previous = icon_button(icons::SKIP_BACK, "Previous");
    let play = gtk::Button::builder()
        .icon_name(icons::PAUSE)
        .tooltip_text("Play/Pause")
        .css_classes(["circular", "suggested-action", "music-play"])
        .build();
    let next = icon_button(icons::SKIP_FORWARD, "Next");
    let repeat = icon_button(icons::REPEAT, "Repeat: off");
    repeat.add_css_class("dim-label");
    let controls = gtk::CenterBox::new();
    let middle = gtk::Box::builder().spacing(18).build();
    middle.append(&previous);
    middle.append(&play);
    middle.append(&next);
    controls.set_start_widget(Some(&shuffle));
    controls.set_center_widget(Some(&middle));
    controls.set_end_widget(Some(&repeat));
    let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
    volume.set_draw_value(false);
    volume.set_hexpand(true);
    let volume_row = gtk::Box::builder().spacing(8).build();
    volume_row.append(&gtk::Image::from_icon_name(icons::VOLUME));
    volume_row.append(&volume);

    let queue_title = gtk::Label::builder()
        .label("Queue")
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["heading"])
        .build();
    let clear = gtk::Button::builder()
        .label("Clear upcoming")
        .css_classes(["flat"])
        .build();
    let queue_header = gtk::Box::builder().margin_top(12).build();
    queue_header.append(&queue_title);
    queue_header.append(&clear);
    let queue = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();

    let collapse = gtk::Button::builder()
        .icon_name(icons::COLLAPSE)
        .tooltip_text("Back to the library")
        .halign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build();
    // One column, as wide as the mini player: what's playing and its
    // controls, then the queue; all of it scrolls together.
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .width_request(PANEL_WIDTH)
        .build();
    body.append(&collapse);
    body.append(&cover_box);
    body.append(&title);
    body.append(&artist);
    body.append(&album);
    body.append(&seek);
    body.append(&times);
    body.append(&controls);
    body.append(&volume_row);
    body.append(&queue_header);
    body.append(&queue);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&body)
        .build();
    sheet.set_sheet(Some(&scrolled));

    let view = Rc::new(View {
        sheet: sheet.clone(),
        bar,
        bar_cover,
        bar_title,
        bar_artist,
        bar_play: bar_play.clone(),
        bar_progress,
        cover,
        title,
        artist: artist.clone(),
        album: album.clone(),
        seek: seek.clone(),
        elapsed,
        total,
        play: play.clone(),
        shuffle: shuffle.clone(),
        repeat: repeat.clone(),
        volume: volume.clone(),
        queue,
        queue_shown: RefCell::new((Vec::new(), 0)),
        shown: RefCell::new(None),
        syncing: Cell::new(false),
        seeking: Cell::new(false),
        moving: Cell::new(None),
        refocus: Cell::new(None),
    });

    // Widgets hold the player weakly: it belongs to the Ui.
    let on = |button: &gtk::Button, act: fn(&MusicPlayer)| {
        let music = music.downgrade();
        button.connect_clicked(move |_| {
            if let Some(music) = music.upgrade() {
                act(&music);
            }
        });
    };
    on(&bar_previous, MusicPlayer::previous);
    on(&bar_play, MusicPlayer::toggle_pause);
    on(&bar_next, MusicPlayer::next);
    on(&previous, MusicPlayer::previous);
    on(&play, MusicPlayer::toggle_pause);
    on(&next, MusicPlayer::next);
    on(&repeat, MusicPlayer::cycle_repeat);
    on(&clear, MusicPlayer::clear_upcoming);
    {
        let sheet = sheet.clone();
        collapse.connect_clicked(move |_| sheet.set_open(false));
    }
    {
        let (music, view) = (music.downgrade(), Rc::downgrade(&view));
        shuffle.connect_toggled(move |button| {
            if let (Some(music), Some(view)) = (music.upgrade(), view.upgrade())
                && !view.syncing.get()
            {
                music.set_shuffle(button.is_active());
            }
        });
    }
    {
        let (music, view) = (music.downgrade(), Rc::downgrade(&view));
        seek.connect_change_value(move |_, _, value| {
            if let (Some(music), Some(view)) = (music.upgrade(), view.upgrade()) {
                view.seeking.set(true);
                music.seek(value);
                view.seeking.set(false);
            }
            glib::Propagation::Proceed
        });
    }
    {
        let (music, view) = (music.downgrade(), Rc::downgrade(&view));
        volume.connect_value_changed(move |scale| {
            if let (Some(music), Some(view)) = (music.upgrade(), view.upgrade())
                && !view.syncing.get()
            {
                music.set_volume(scale.value());
            }
        });
    }
    for button in [&artist, &album] {
        let (ui, sheet) = (ui.downgrade(), sheet.downgrade());
        button.connect_clicked(move |button| open_link(&ui, &sheet, button));
    }

    {
        let (ui, view, weak_music) = (ui.downgrade(), view.clone(), music.downgrade());
        music.on_change(move || {
            if let (Some(ui), Some(music)) = (ui.upgrade(), weak_music.upgrade()) {
                refresh(&ui, &music, &view);
            }
        });
    }
    {
        let (view, music) = (view.clone(), music.downgrade());
        glib::timeout_add_local(TICK, move || match music.upgrade() {
            Some(music) => {
                if music.is_active() {
                    tick(&music, &view);
                }
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
    }
    MusicPanel {
        view,
        music: music.downgrade(),
    }
}

/// Re-reads everything from the player after a change.
fn refresh(ui: &Ui, music: &MusicPlayer, view: &Rc<View>) {
    let active = music.is_active();
    if active && view.sheet.bottom_bar().is_none() {
        view.sheet.set_bottom_bar(Some(&view.bar));
    } else if !active && view.sheet.bottom_bar().is_some() {
        view.sheet.set_open(false);
        view.sheet.set_bottom_bar(None::<&gtk::Widget>);
    }
    view.sheet.set_can_open(active);
    if !active {
        view.shown.replace(None);
        return;
    }

    view.syncing.set(true);
    let icon = if music.is_paused() {
        icons::PLAY
    } else {
        icons::PAUSE
    };
    view.bar_play.set_icon_name(icon);
    view.play.set_icon_name(icon);
    let shuffled = music.queue().shuffled();
    view.shuffle.set_active(shuffled);
    let (repeat_icon, repeat_tip) = match music.repeat() {
        Repeat::Off => (icons::REPEAT, "Repeat: off"),
        Repeat::All => (icons::REPEAT, "Repeat: all"),
        Repeat::One => (icons::REPEAT_ONE, "Repeat: this track"),
    };
    view.repeat.set_icon_name(repeat_icon);
    view.repeat.set_tooltip_text(Some(repeat_tip));
    if music.repeat() == Repeat::Off {
        view.repeat.add_css_class("dim-label");
    } else {
        view.repeat.remove_css_class("dim-label");
    }
    view.volume.set_value(music.volume());
    view.syncing.set(false);

    if let Some(item) = music.current()
        && view.shown.borrow().as_deref() != Some(item.id.as_str())
    {
        show_track(ui, view, &item);
        view.shown.replace(Some(item.id.clone()));
    }
    rebuild_queue(music, view);
    tick(music, view);
}

fn show_track(ui: &Ui, view: &View, item: &BaseItem) {
    let artist = item.artist().unwrap_or_default().to_string();
    view.bar_title.set_label(&item.name);
    view.bar_artist.set_label(&artist);
    view.title.set_label(&item.name);
    images::load(ui, &view.bar_cover, item.poster(), BAR_COVER as u32 * 2);
    images::load(ui, &view.cover, item.poster(), SHEET_COVER as u32 * 2);

    let artist_link = item.artist_items.first().map(|a| BaseItem {
        id: a.id.clone(),
        name: a.name.clone(),
        item_type: "MusicArtist".into(),
        ..Default::default()
    });
    set_link(&view.artist, artist_link.as_ref(), &artist);
    let album_link = item.album_id.as_ref().map(|id| BaseItem {
        id: id.clone(),
        name: item.album.clone().unwrap_or_default(),
        item_type: "MusicAlbum".into(),
        ..Default::default()
    });
    set_link(
        &view.album,
        album_link.as_ref(),
        item.album.as_deref().unwrap_or_default(),
    );
}

const LINK_KEY: &str = "embyclientplus-music-link";

fn set_link(button: &gtk::Button, target: Option<&BaseItem>, text: &str) {
    button.set_label(text);
    button.set_visible(!text.is_empty());
    button.set_sensitive(target.is_some());
    // SAFETY: this key is only ever stored and read as `Option<BaseItem>`.
    unsafe { button.set_data(LINK_KEY, target.cloned()) };
}

fn open_link(ui: &WeakUi, sheet: &glib::WeakRef<adw::BottomSheet>, button: &gtk::Button) {
    // SAFETY: see `set_link`; cloned out immediately.
    let target = unsafe {
        button
            .data::<Option<BaseItem>>(LINK_KEY)
            .and_then(|t| t.as_ref().clone())
    };
    if let (Some(ui), Some(target)) = (ui.upgrade(), target) {
        if let Some(sheet) = sheet.upgrade() {
            sheet.set_open(false);
        }
        ui.open(&target);
    }
}

/// Position and duration, twice a second.
fn tick(music: &MusicPlayer, view: &View) {
    let position = music.position().unwrap_or(0.0);
    let duration = music.duration().unwrap_or(0.0);
    view.bar_progress.set_fraction(if duration > 0.0 {
        (position / duration).clamp(0.0, 1.0)
    } else {
        0.0
    });
    if !view.seeking.get() {
        view.seek.set_range(0.0, duration.max(1.0));
        view.seek.set_value(position);
    }
    let ticks = |seconds: f64| (seconds * TICKS_PER_SECOND as f64) as i64;
    view.elapsed.set_label(&format_timestamp(ticks(position)));
    view.total.set_label(&format_timestamp(ticks(duration)));
}

fn rebuild_queue(music: &MusicPlayer, view: &View) {
    let (ids, position) = {
        let queue = music.queue();
        (
            queue.entries().map(|i| i.id.clone()).collect::<Vec<_>>(),
            queue.position(),
        )
    };
    if *view.queue_shown.borrow() == (ids.clone(), position) {
        return;
    }
    view.queue.remove_all();
    let entries: Vec<BaseItem> = music.queue().entries().cloned().collect();
    let last = entries.len().saturating_sub(1);
    for (index, item) in entries.iter().enumerate() {
        let row = queue_row(music, item, index, position, last);
        if view.moving.get() == Some(index) {
            row.add_css_class("queue-moving");
        }
        view.queue.append(&row);
    }
    view.queue_shown.replace((ids, position));
    // A controller move rebuilt the list: the cursor stays on that row.
    if let Some(index) = view.refocus.take()
        && let Some(row) = view.queue.row_at_index(index as i32)
    {
        row.grab_focus();
    }
}

type QueueEdit = Box<dyn Fn(&MusicPlayer)>;

fn queue_row(
    music: &MusicPlayer,
    item: &BaseItem,
    index: usize,
    current: usize,
    last: usize,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&item.name))
        .subtitle(glib::markup_escape_text(item.artist().unwrap_or_default()))
        .activatable(index != current)
        .build();
    let marker = gtk::Image::builder()
        .icon_name(icons::SONGS)
        .opacity(if index == current { 1.0 } else { 0.0 })
        .build();
    if index == current {
        row.add_css_class("music-current");
    }
    row.add_prefix(&marker);
    add_drag_and_drop(music, &row, index);
    if index != current {
        let actions: [(&str, &str, bool, QueueEdit); 3] = [
            (
                icons::MOVE_UP,
                "Move up",
                index > 0,
                Box::new(move |m| m.shift(index, true)),
            ),
            (
                icons::MOVE_DOWN,
                "Move down",
                index < last,
                Box::new(move |m| m.shift(index, false)),
            ),
            (
                icons::REMOVE,
                "Remove",
                true,
                Box::new(move |m| m.remove(index)),
            ),
        ];
        for (icon, tip, sensitive, act) in actions {
            let button = icon_button(icon, tip);
            button.set_sensitive(sensitive);
            // For the mouse; the controller moves row to row and uses X to
            // move one (see `MusicPanel`).
            button.set_focusable(false);
            let music = music.downgrade();
            button.connect_clicked(move |_| {
                if let Some(music) = music.upgrade() {
                    act(&music);
                }
            });
            row.add_suffix(&button);
        }
        let music = music.downgrade();
        row.connect_activated(move |_| {
            if let Some(music) = music.upgrade() {
                music.jump(index);
            }
        });
    }
    row
}

/// Mouse reordering: drag a queue row onto another to move it there.
fn add_drag_and_drop(music: &MusicPlayer, row: &adw::ActionRow, index: usize) {
    let drag = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    drag.connect_prepare(move |source, _, _| {
        if let Some(widget) = source.widget() {
            // The row itself follows the pointer.
            source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&widget))), 0, 0);
        }
        Some(gtk::gdk::ContentProvider::for_value(
            &(index as u32).to_value(),
        ))
    });
    row.add_controller(drag);

    let drop = gtk::DropTarget::new(u32::static_type(), gtk::gdk::DragAction::MOVE);
    let music = music.downgrade();
    drop.connect_drop(move |_, value, _, _| {
        let (Ok(from), Some(music)) = (value.get::<u32>(), music.upgrade()) else {
            return false;
        };
        music.move_item(from as usize, index);
        true
    });
    row.add_controller(drop);
}

fn cover_frame(picture: &gtk::Picture, size: i32) -> gtk::Overlay {
    let frame = gtk::Overlay::builder()
        .child(&fixed_picture(picture, size, size))
        .overflow(gtk::Overflow::Hidden)
        .valign(gtk::Align::Center)
        .css_classes(["card"])
        .build();
    let placeholder = placeholder_for(picture, icons::SONGS);
    placeholder.set_pixel_size((size / 2).min(48));
    frame.add_overlay(&placeholder);
    frame
}

fn label(classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(classes.to_vec())
        .build()
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build()
}

fn link_button() -> gtk::Button {
    gtk::Button::builder()
        .halign(gtk::Align::Center)
        .css_classes(["flat"])
        .visible(false)
        .build()
}

impl MusicPanel {
    pub fn is_open(&self) -> bool {
        self.view.sheet.is_open()
    }

    /// Opens the panel (when music is playing), cursor on Play/Pause.
    pub fn open(&self) -> bool {
        let sheet = &self.view.sheet;
        if !sheet.can_open() {
            return false;
        }
        sheet.set_open(true);
        self.focus_default();
        true
    }

    pub fn close(&self) {
        self.end_move();
        self.view.sheet.set_open(false);
    }

    /// Puts the cursor on Play/Pause, unless it's already in the panel.
    pub fn focus_default(&self) {
        let play = self.view.play.downgrade();
        let sheet = self.view.sheet.downgrade();
        glib::idle_add_local_once(move || {
            let (Some(play), Some(sheet)) = (play.upgrade(), sheet.upgrade()) else {
                return;
            };
            let inside = sheet
                .sheet()
                .zip(sheet.root().and_then(|root| root.focus()))
                .is_some_and(|(panel, focus)| focus.is_ancestor(&panel) && focus.is_mapped());
            if !inside {
                play.grab_focus();
            }
        });
    }

    /// Whether `widget` is inside the panel.
    pub fn contains(&self, widget: &gtk::Widget) -> bool {
        self.view
            .sheet
            .sheet()
            .is_some_and(|panel| widget.is_ancestor(&panel))
    }

    /// The queue row being moved, if any.
    pub fn moving(&self) -> Option<usize> {
        self.view.moving.get()
    }

    /// Runs a music-panel controller action; false when it doesn't apply.
    pub fn action(&self, action: crate::controller::Action, focus: Option<&gtk::Widget>) -> bool {
        use crate::controller::Action;
        let Some(music) = self.music.upgrade() else {
            return false;
        };
        match action {
            Action::MusicPlayPause => music.toggle_pause(),
            Action::MusicPrevious => music.previous(),
            Action::MusicNext => music.next(),
            Action::CloseMusic => self.close(),
            Action::MoveInQueue => return self.start_move(focus),
            _ => return false,
        }
        true
    }

    /// X on a queue row: pick it up to move with ↑/↓ (not the playing one).
    fn start_move(&self, focus: Option<&gtk::Widget>) -> bool {
        let Some(row) = focus.and_then(|f| {
            f.ancestor(gtk::ListBoxRow::static_type())
                .and_downcast::<gtk::ListBoxRow>()
                .filter(|row| row.parent().as_ref() == Some(self.view.queue.upcast_ref()))
        }) else {
            return false;
        };
        let Some(music) = self.music.upgrade() else {
            return false;
        };
        let index = row.index().max(0) as usize;
        if index >= music.queue().entries().count() {
            return false;
        }
        self.view.moving.set(Some(index));
        row.add_css_class("queue-moving");
        true
    }

    /// ↑/↓ while moving: the row trades places with its neighbour.
    pub fn move_step(&self, up: bool) {
        let (Some(index), Some(music)) = (self.view.moving.get(), self.music.upgrade()) else {
            return;
        };
        let last = music.queue().entries().count().saturating_sub(1);
        let target = if up {
            index.checked_sub(1)
        } else {
            (index < last).then_some(index + 1)
        };
        // Moves pass the playing track too; it keeps playing.
        let Some(target) = target else {
            return;
        };
        self.view.moving.set(Some(target));
        self.view.refocus.set(Some(target));
        tracing::info!("queue: moved {index} to {target}");
        music.shift(index, up);
    }

    /// Puts the moved row down (A, B or X again).
    pub fn end_move(&self) {
        if self.view.moving.take().is_some() {
            let mut child = self.view.queue.first_child();
            while let Some(row) = child {
                row.remove_css_class("queue-moving");
                child = row.next_sibling();
            }
        }
    }
}
