//! The navigation page hosting the video and its on-screen controls.
//! Created once per process: mpv's event handler can only be registered
//! once (see `player`), and it reaches the page through `CURRENT`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::{format_timestamp, images};
use crate::emby::EmbyClient;
use crate::emby::models::BaseItem;
use crate::playback::{PlaybackSession, QUALITIES, Quality, TICKS_PER_SECOND, TrackEntry};
use crate::player::compositor::BarFill;
use crate::player::{
    self, Aspect, END_FILE_REASON_EOF, END_FILE_REASON_ERROR, Player, PlayerEvent, TrackKind,
};

/// OSD hides after this long without pointer movement while playing.
const OSD_TIMEOUT: Duration = Duration::from_secs(3);
/// Position/markers/Up Next refresh rate while the page is shown.
const TICK_INTERVAL: Duration = Duration::from_millis(250);
const UP_NEXT_COUNTDOWN: Duration = Duration::from_secs(10);
const SEEK_STEP: i32 = 10;
/// One zoom step, in mpv's log2 units (about 7%).
const ZOOM_STEP: f64 = 0.1;
/// How often the SVP button re-checks whether SVP attached.
const SVP_CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// Holding seek this long starts scrubbing (a tap seeks right away).
const HOLD_TO_SCRUB: Duration = Duration::from_secs(1);
/// A scrub lands this long after the target last moved.
const SCRUB_COMMIT: Duration = Duration::from_secs(2);
/// Longest gap between auto-repeats of a held key (keyboard repeat
/// delay is usually 250–600 ms, the rate 20–40 per second).
const KEY_REPEAT_GAP: Duration = Duration::from_millis(1000);
/// How long the volume pop-up stays after the last change.
const VOLUME_OSD_TIME: Duration = Duration::from_millis(1500);
/// Volume changes are reported to Emby once they settle.
const VOLUME_REPORT_DELAY: Duration = Duration::from_millis(800);
const SVP_STATE_CLASSES: [&str; 3] = ["svp-active", "svp-waiting", "svp-missing"];
const VOLUME_STEP: f64 = 5.0;
/// Subtitle action target meaning "off".
const SUBTITLES_OFF_ID: i64 = -1;

thread_local! {
    /// Titles already told that HDR keeps SVP off (once per run).
    static HDR_NOTICED: RefCell<std::collections::HashSet<String>> = RefCell::default();
    /// The page mpv events are delivered to (main thread only).
    static CURRENT: RefCell<Weak<Inner>> = const { RefCell::new(Weak::new()) };
}

/// What the page asks of whoever is driving playback (the `Ui`).
pub struct Handlers {
    /// Play another item (previous/next episode) from its resume point.
    pub play: Box<dyn Fn(&BaseItem)>,
    pub change_quality: Box<dyn Fn(Quality)>,
    /// The page was left; stop and report.
    pub stopped: Box<dyn Fn()>,
    pub toast: Box<dyn Fn(&str)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum UpNext {
    Hidden,
    Counting(Instant),
    /// The user dismissed it; don't autoplay this item's successor.
    Cancelled,
}

#[derive(Clone)]
pub struct PlayerPage {
    inner: Rc<Inner>,
}

struct Inner {
    page: adw::NavigationPage,
    player: Player,
    video: gtk::GLArea,
    osd: Osd,
    actions: gio::SimpleActionGroup,
    session: RefCell<Option<Rc<PlaybackSession>>>,
    /// Shared so a callback can run without holding the cell's borrow:
    /// callbacks restart playback, which installs new handlers.
    handlers: RefCell<Option<Rc<Handlers>>>,
    quality: Cell<Quality>,
    up_next: Cell<UpNext>,
    /// When the user last moved the seek bar; position updates pause briefly.
    seeked_at: Cell<Option<Instant>>,
    last_motion: Cell<Option<(f64, f64)>>,
    hide_timer: RefCell<Option<glib::SourceId>>,
    tick: RefCell<Option<glib::SourceId>>,
    volume_report: RefCell<Option<glib::SourceId>>,
    /// Last volume/mute seen, so only real changes pop up the volume OSD.
    volume_seen: Cell<Option<(i64, bool)>>,
    /// Start pressed: the controller moves through the OSD's buttons
    /// instead of seeking, and the OSD stays up.
    controls_mode: Cell<bool>,
    /// The item last prepared, to tell a restart (quality change) from a
    /// new title.
    prepared_id: RefCell<Option<String>>,
    /// Held seek (controller/arrow repeat): where it's headed. mpv seeks
    /// once, when the presses stop, instead of on every repeat.
    scrub_target: Cell<Option<f64>>,
    /// When the current seek press began (for hold detection).
    hold_started: Cell<Option<Instant>>,
    /// The arrow key held down (no release since its press) and when it
    /// last fired, to tell auto-repeat from fresh presses.
    key_held: Cell<Option<(gdk::Key, Instant)>>,
    scrub_timer: RefCell<Option<glib::SourceId>>,
    volume_osd_timer: RefCell<Option<glib::SourceId>>,
    svp_checked_at: Cell<Option<Instant>>,
    /// The "SVP Manager isn't running" toast shows once per item.
    svp_warned: Cell<bool>,
}

struct Osd {
    top: gtk::Revealer,
    bottom: gtk::Revealer,
    title: gtk::Label,
    subtitle: gtk::Label,
    seek: gtk::Scale,
    elapsed: gtk::Label,
    remaining: gtk::Label,
    play: gtk::Button,
    previous: gtk::Button,
    next: gtk::Button,
    volume: gtk::Scale,
    volume_button: gtk::MenuButton,
    audio: gtk::MenuButton,
    subtitles: gtk::MenuButton,
    quality: gtk::MenuButton,
    picture: gtk::MenuButton,
    svp: gtk::ToggleButton,
    fullscreen: gtk::Button,
    skip: gtk::Button,
    up_next: gtk::Box,
    up_next_picture: gtk::Picture,
    up_next_title: gtk::Label,
    up_next_countdown: gtk::Label,
    up_next_play: gtk::Button,
    up_next_cancel: gtk::Button,
    /// Frame, chapter and time over the seek bar while scrubbing.
    preview: super::scrub_preview::ScrubPreview,
    /// Pops up on volume changes, even with the OSD hidden (controller).
    volume_osd: gtk::Revealer,
    volume_osd_icon: gtk::Image,
    volume_osd_level: gtk::ProgressBar,
    volume_osd_label: gtk::Label,
}

impl PlayerPage {
    pub fn new(player: Player) -> Self {
        let video = player::video_area::new(player);
        let (osd, overlay) = Osd::build(&video);
        let page = adw::NavigationPage::builder()
            .child(&overlay)
            .title("Player")
            .tag("player")
            .build();
        let actions = gio::SimpleActionGroup::new();
        page.insert_action_group("player", Some(&actions));
        let inner = Rc::new(Inner {
            page,
            player,
            video,
            osd,
            actions,
            session: RefCell::new(None),
            handlers: RefCell::new(None),
            quality: Cell::new(Quality::Original),
            up_next: Cell::new(UpNext::Hidden),
            seeked_at: Cell::new(None),
            last_motion: Cell::new(None),
            hide_timer: RefCell::new(None),
            tick: RefCell::new(None),
            volume_report: RefCell::new(None),
            volume_seen: Cell::new(None),
            controls_mode: Cell::new(false),
            prepared_id: RefCell::new(None),
            scrub_target: Cell::new(None),
            hold_started: Cell::new(None),
            key_held: Cell::new(None),
            scrub_timer: RefCell::new(None),
            volume_osd_timer: RefCell::new(None),
            svp_checked_at: Cell::new(None),
            svp_warned: Cell::new(false),
        });
        CURRENT.with(|current| *current.borrow_mut() = Rc::downgrade(&inner));
        inner.connect(&overlay);

        player.on_event(|event| {
            glib::MainContext::default().invoke(move || {
                if crate::music::dispatch(&event) {
                    return;
                }
                if let Some(inner) = CURRENT.with(|current| current.borrow().upgrade()) {
                    inner.handle(event);
                }
            });
        });
        PlayerPage { inner }
    }

    pub fn page(&self) -> &adw::NavigationPage {
        &self.inner.page
    }

    /// Whether one of the OSD's menus (audio, subtitles, ...) is open, so
    /// controller directions should move through it instead of seeking.
    pub fn menu_open(&self) -> bool {
        self.inner.osd.menu_open()
    }

    /// Whether the controller is moving through the OSD's buttons.
    pub fn controls_mode(&self) -> bool {
        self.inner.controls_mode.get()
    }

    /// Leaves the OSD-buttons mode; the OSD hides again as usual.
    pub fn leave_controls_mode(&self) {
        let inner = &self.inner;
        inner.controls_mode.set(false);
        if let Some(window) = inner.window() {
            gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
        }
        inner.show_osd();
    }

    /// Runs a player-context controller action.
    /// `repeat`: a held direction repeating (held seeks scrub instead).
    pub fn controller_action(&self, action: crate::controller::Action, repeat: bool) {
        use crate::controller::Action;
        let inner = &self.inner;
        let player = inner.player;
        match action {
            Action::PlayPause => warn(player.toggle_pause()),
            Action::Leave => inner.pop(),
            Action::SeekBack => {
                inner.show_osd();
                inner.seek_by(-SEEK_STEP, repeat);
            }
            Action::SeekForward => {
                inner.show_osd();
                inner.seek_by(SEEK_STEP, repeat);
            }
            Action::VolumeUp => warn(player.set_volume(player.volume() + VOLUME_STEP)),
            Action::VolumeDown => warn(player.set_volume(player.volume() - VOLUME_STEP)),
            Action::PreviousEpisode => inner.play_neighbour(false),
            Action::NextEpisode => inner.play_neighbour(true),
            Action::PreviousChapter => inner.seek_chapter(false),
            Action::NextChapter => inner.seek_chapter(true),
            Action::AudioMenu => {
                inner.show_osd();
                inner.osd.audio.popup();
            }
            Action::SubtitleMenu => {
                inner.show_osd();
                inner.osd.subtitles.popup();
            }
            // The OSD stays up and the controller moves through its buttons.
            Action::Skip => {
                if inner.osd.skip.is_visible() {
                    inner.skip();
                }
            }
            Action::ShowControls => {
                inner.controls_mode.set(true);
                inner.show_osd();
                inner.osd.play.grab_focus();
                return;
            }
            _ => return,
        }
        inner.show_osd();
    }

    /// Readies the page for `item` before its stream is negotiated.
    pub fn prepare(&self, item: &BaseItem, quality: Quality, handlers: Handlers) {
        self.inner.osd.preview.clear();
        // Restarting the same title (a quality change) keeps the
        // controller on the OSD buttons, back on the quality one.
        let same_title = self
            .inner
            .prepared_id
            .replace(Some(item.id.clone()))
            .as_deref()
            == Some(item.id.as_str());
        if same_title && self.inner.controls_mode.get() {
            let quality_button = self.inner.osd.quality.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(button) = quality_button.upgrade() {
                    button.grab_focus();
                }
            });
        } else {
            self.inner.controls_mode.set(false);
        }
        // A scrub still waiting to land belongs to the previous video.
        self.inner.scrub_target.set(None);
        if let Some(pending) = self.inner.scrub_timer.take() {
            pending.remove();
        }
        // mpv's volume carries over between files (and from music), and
        // no change event comes until it changes again.
        self.inner.sync_volume();
        let inner = &self.inner;
        inner.session.replace(None);
        inner.handlers.replace(Some(Rc::new(handlers)));
        inner.quality.set(quality);
        inner.up_next.set(UpNext::Hidden);
        inner.svp_warned.set(false);
        inner.svp_checked_at.set(None);
        inner.page.set_title(&item.episode_label());
        let (title, subtitle) = titles(item);
        inner.osd.title.set_label(&title);
        inner.osd.subtitle.set_label(&subtitle);
        inner.osd.subtitle.set_visible(!subtitle.is_empty());
        inner.apply_picture(Aspect::Fit, 0.0, false);
        inner.osd.seek.set_range(0.0, 1.0);
        inner.osd.seek.set_value(0.0);
        inner.osd.seek.clear_marks();
        inner.osd.previous.set_sensitive(false);
        inner.osd.next.set_sensitive(false);
        inner.osd.up_next.set_visible(false);
        inner.osd.skip.set_visible(false);
        inner.rebuild_menus();
        inner.show_osd();
    }

    /// Hooks up a started session: markers, neighbours, menus, SVP state.
    pub fn attach(&self, session: Rc<PlaybackSession>, client: &std::sync::Arc<EmbyClient>) {
        let inner = &self.inner;
        inner
            .osd
            .preview
            .load(client, &session.item, session.trickplay_source());
        inner.osd.previous.set_sensitive(session.previous.is_some());
        inner.osd.next.set_sensitive(session.next.is_some());
        inner.osd.svp.set_active(session.svp_enabled());
        if session.hdr && !session.svp_enabled() {
            let first_time =
                HDR_NOTICED.with(|seen| seen.borrow_mut().insert(session.item.id.clone()));
            if first_time && let Some(handlers) = inner.handlers() {
                (handlers.toast)("HDR: SVP is off for this title (turn it on with the SVP button)");
            }
        }
        if let Some(next) = &session.next {
            inner.osd.up_next_title.set_label(&next.episode_label());
            images::load_with_client(client, &inner.osd.up_next_picture, next.landscape(), 384);
        }
        let (aspect, zoom) = session.picture();
        inner.session.replace(Some(session));
        inner.apply_picture(aspect, zoom, false);
        inner.refresh_duration();
        inner.rebuild_menus();
    }
}

impl Osd {
    fn build(video: &gtk::GLArea) -> (Osd, gtk::Overlay) {
        let icon_button = |icon: &str, tooltip: &str| {
            gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .css_classes(["flat", "circular"])
                .build()
        };
        let menu_button = |icon: &str, tooltip: &str| {
            gtk::MenuButton::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .direction(gtk::ArrowType::Up)
                .css_classes(["flat", "circular"])
                .build()
        };

        // Top: back + title.
        let back = icon_button(crate::ui::icons::BACK, "Back");
        back.set_action_name(Some("player.back"));
        let title = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["title-4"])
            .build();
        let subtitle = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label"])
            .build();
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 0);
        titles.append(&title);
        titles.append(&subtitle);
        let top_bar = gtk::Box::builder()
            .spacing(12)
            .css_classes(["osd", "player-bar"])
            .build();
        top_bar.append(&back);
        top_bar.append(&titles);
        let top = gtk::Revealer::builder()
            .child(&top_bar)
            .valign(gtk::Align::Start)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .reveal_child(true)
            .build();

        // Bottom: seek row + buttons row.
        let elapsed = gtk::Label::builder()
            .label("0:00")
            .css_classes(["numeric"])
            .build();
        let remaining = gtk::Label::builder()
            .label("0:00")
            .css_classes(["numeric"])
            .build();
        let seek = gtk::Scale::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .draw_value(false)
            .build();
        seek.set_range(0.0, 1.0);
        let seek_row = gtk::Box::builder().spacing(12).build();
        seek_row.append(&elapsed);
        seek_row.append(&seek);
        seek_row.append(&remaining);

        let previous = icon_button(crate::ui::icons::SKIP_BACK, "Previous episode (P)");
        let play = icon_button(crate::ui::icons::PAUSE, "Play/Pause (Space)");
        play.add_css_class("large-button");
        let next = icon_button(crate::ui::icons::SKIP_FORWARD, "Next episode (N)");
        let volume = gtk::Scale::builder()
            .orientation(gtk::Orientation::Horizontal)
            .width_request(180)
            .draw_value(false)
            .build();
        volume.set_range(0.0, 130.0);
        volume.add_mark(100.0, gtk::PositionType::Bottom, None);
        let mute = gtk::Button::builder()
            .icon_name(crate::ui::icons::MUTED)
            .tooltip_text("Mute (M)")
            .action_name("player.mute")
            .css_classes(["flat"])
            .build();
        let volume_box = gtk::Box::builder().spacing(6).build();
        volume_box.append(&mute);
        volume_box.append(&volume);
        let volume_button = menu_button(crate::ui::icons::VOLUME, "Volume");
        volume_button.set_popover(Some(&gtk::Popover::builder().child(&volume_box).build()));
        let audio = menu_button(crate::ui::icons::AUDIO, "Audio");
        let subtitles = menu_button(crate::ui::icons::SUBTITLES, "Subtitles");
        let quality = menu_button(crate::ui::icons::QUALITY, "Quality");
        let picture = menu_button(crate::ui::icons::PICTURE, "Picture");
        picture.set_menu_model(Some(&picture_menu()));
        let svp = gtk::ToggleButton::builder()
            .label("SVP")
            .tooltip_text("Frame interpolation through SVP (remembered for this title)")
            .css_classes(["flat"])
            .build();
        let fullscreen = icon_button(crate::ui::icons::FULLSCREEN, "Fullscreen (F)");

        let buttons = gtk::CenterBox::new();
        let start = gtk::Box::builder().spacing(6).build();
        start.append(&volume_button);
        let middle = gtk::Box::builder().spacing(12).build();
        middle.append(&previous);
        middle.append(&play);
        middle.append(&next);
        let end = gtk::Box::builder().spacing(6).build();
        end.append(&audio);
        end.append(&subtitles);
        end.append(&quality);
        end.append(&picture);
        end.append(&svp);
        end.append(&fullscreen);
        buttons.set_start_widget(Some(&start));
        buttons.set_center_widget(Some(&middle));
        buttons.set_end_widget(Some(&end));

        let bottom_bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .css_classes(["osd", "player-bar"])
            .build();
        bottom_bar.append(&seek_row);
        bottom_bar.append(&buttons);
        let bottom = gtk::Revealer::builder()
            .child(&bottom_bar)
            .valign(gtk::Align::End)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .reveal_child(true)
            .build();

        // Skip Intro / Skip Credits.
        let skip = gtk::Button::builder()
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_end(32)
            .margin_bottom(140)
            .css_classes(["pill", "osd", "skip-button"])
            .visible(false)
            .build();

        // Up Next card.
        let up_next_picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Cover)
            .build();
        let picture_frame = gtk::Overlay::builder()
            .child(&super::fixed_picture(&up_next_picture, 192, 108))
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["card"])
            .build();
        let up_next_title = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .max_width_chars(28)
            .css_classes(["heading"])
            .build();
        let up_next_countdown = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        let up_next_play = gtk::Button::builder()
            .label("Play Now")
            .css_classes(["suggested-action", "pill"])
            .build();
        let up_next_cancel = gtk::Button::builder()
            .label("Cancel")
            .css_classes(["pill"])
            .build();
        let up_next_buttons = gtk::Box::builder().spacing(6).build();
        up_next_buttons.append(&up_next_play);
        up_next_buttons.append(&up_next_cancel);
        let up_next_text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        up_next_text.append(
            &gtk::Label::builder()
                .label("Up Next")
                .xalign(0.0)
                .css_classes(["caption-heading", "dim-label"])
                .build(),
        );
        up_next_text.append(&up_next_title);
        up_next_text.append(&up_next_countdown);
        up_next_text.append(&up_next_buttons);
        let up_next = gtk::Box::builder()
            .spacing(12)
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_end(32)
            .margin_bottom(140)
            .css_classes(["osd", "up-next"])
            .visible(false)
            .build();
        up_next.append(&picture_frame);
        up_next.append(&up_next_text);

        let overlay = gtk::Overlay::builder().child(video).build();
        overlay.add_overlay(&top);
        overlay.add_overlay(&bottom);
        overlay.add_overlay(&skip);
        overlay.add_overlay(&up_next);

        let volume_osd_icon = gtk::Image::builder()
            .icon_name(crate::ui::icons::VOLUME)
            .pixel_size(24)
            .build();
        let volume_osd_level = gtk::ProgressBar::builder()
            .width_request(200)
            .valign(gtk::Align::Center)
            .build();
        let volume_osd_label = gtk::Label::builder()
            .width_chars(5)
            .xalign(1.0)
            .css_classes(["numeric", "heading"])
            .build();
        let volume_osd_box = gtk::Box::builder()
            .spacing(12)
            .css_classes(["osd", "volume-osd"])
            .build();
        volume_osd_box.append(&volume_osd_icon);
        volume_osd_box.append(&volume_osd_level);
        volume_osd_box.append(&volume_osd_label);
        let volume_osd = gtk::Revealer::builder()
            .child(&volume_osd_box)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Start)
            .margin_top(96)
            .can_target(false)
            .build();
        overlay.add_overlay(&volume_osd);
        let preview = super::scrub_preview::ScrubPreview::new();
        overlay.add_overlay(preview.widget());

        let osd = Osd {
            top,
            bottom,
            title,
            subtitle,
            seek,
            elapsed,
            remaining,
            play,
            previous,
            next,
            volume,
            volume_button,
            audio,
            subtitles,
            quality,
            picture,
            svp,
            fullscreen,
            skip,
            up_next,
            up_next_picture,
            up_next_title,
            up_next_countdown,
            up_next_play,
            up_next_cancel,
            preview,
            volume_osd,
            volume_osd_icon,
            volume_osd_level,
            volume_osd_label,
        };
        (osd, overlay)
    }

    fn menu_open(&self) -> bool {
        [
            &self.volume_button,
            &self.audio,
            &self.subtitles,
            &self.quality,
            &self.picture,
        ]
        .iter()
        .any(|button| button.is_active())
    }
}

impl Inner {
    fn connect(self: &Rc<Self>, overlay: &gtk::Overlay) {
        let weak = Rc::downgrade(self);
        let with = move |f: fn(&Rc<Inner>)| {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    f(&inner);
                }
            }
        };

        // Actions (also reachable from keys and menus).
        let simple = |name: &str, callback: Box<dyn Fn()>| {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(move |_, _| callback());
            self.actions.add_action(&action);
        };
        simple("back", Box::new(with(|inner| inner.leave())));
        simple(
            "mute",
            Box::new(with(|inner| warn(inner.player.toggle_mute()))),
        );

        let weak = Rc::downgrade(self);
        let audio = gio::SimpleAction::new_stateful(
            "audio",
            Some(glib::VariantTy::INT64),
            &0i64.to_variant(),
        );
        audio.connect_activate(glib::clone!(
            #[strong]
            weak,
            move |action, value| {
                if let (Some(inner), Some(id)) =
                    (weak.upgrade(), value.and_then(|v| v.get::<i64>()))
                {
                    action.set_state(&id.to_variant());
                    if let Some(session) = inner.session() {
                        session.select_audio(id);
                    }
                }
            }
        ));
        self.actions.add_action(&audio);
        let subtitle = gio::SimpleAction::new_stateful(
            "subtitle",
            Some(glib::VariantTy::INT64),
            &SUBTITLES_OFF_ID.to_variant(),
        );
        subtitle.connect_activate(glib::clone!(
            #[strong]
            weak,
            move |action, value| {
                if let (Some(inner), Some(id)) =
                    (weak.upgrade(), value.and_then(|v| v.get::<i64>()))
                {
                    action.set_state(&id.to_variant());
                    if let Some(session) = inner.session() {
                        session.select_subtitle((id != SUBTITLES_OFF_ID).then_some(id));
                    }
                }
            }
        ));
        self.actions.add_action(&subtitle);
        let quality = gio::SimpleAction::new_stateful(
            "quality",
            Some(glib::VariantTy::INT32),
            &0i32.to_variant(),
        );
        quality.connect_activate(glib::clone!(
            #[strong]
            weak,
            move |action, value| {
                let (Some(inner), Some(index)) =
                    (weak.upgrade(), value.and_then(|v| v.get::<i32>()))
                else {
                    return;
                };
                let Some(&chosen) = QUALITIES.get(index as usize) else {
                    return;
                };
                if chosen == inner.quality.get() {
                    return;
                }
                action.set_state(&index.to_variant());
                if let Some(handlers) = inner.handlers() {
                    (handlers.change_quality)(chosen);
                }
            }
        ));
        self.actions.add_action(&quality);

        // Picture: bar fill (global), aspect and zoom (per title).
        let fill_names = ["off", "blur", "glow"];
        let current_fill = fill_names[self.player.bar_fill() as usize].to_variant();
        let fill = gio::SimpleAction::new_stateful(
            "bar-fill",
            Some(glib::VariantTy::STRING),
            &current_fill,
        );
        fill.connect_activate(glib::clone!(
            #[strong]
            weak,
            move |action, value| {
                let (Some(inner), Some(name)) =
                    (weak.upgrade(), value.and_then(|v| v.get::<String>()))
                else {
                    return;
                };
                action.set_state(&name.to_variant());
                let fill = bar_fill_from_name(&name);
                warn(inner.player.set_bar_fill(fill));
                inner.video.queue_render();
                let saved = name.clone();
                if let Err(e) = crate::config::Settings::update(|s| s.playback.bar_fill = saved) {
                    tracing::warn!("{e:#}");
                }
            }
        ));
        self.actions.add_action(&fill);
        let aspect = gio::SimpleAction::new_stateful(
            "aspect",
            Some(glib::VariantTy::STRING),
            &Aspect::Fit.name().to_variant(),
        );
        aspect.connect_activate(glib::clone!(
            #[strong]
            weak,
            move |action, value| {
                let (Some(inner), Some(name)) =
                    (weak.upgrade(), value.and_then(|v| v.get::<String>()))
                else {
                    return;
                };
                let Some(chosen) = Aspect::from_name(&name) else {
                    return;
                };
                action.set_state(&name.to_variant());
                inner.apply_picture(chosen, inner.player.user_zoom(), true);
            }
        ));
        self.actions.add_action(&aspect);
        for (name, step) in [
            ("zoom-in", ZOOM_STEP),
            ("zoom-out", -ZOOM_STEP),
            ("zoom-reset", 0.0),
        ] {
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(glib::clone!(
                #[strong]
                weak,
                move |_, _| {
                    let Some(inner) = weak.upgrade() else { return };
                    let zoom = if step == 0.0 {
                        0.0
                    } else {
                        inner.player.user_zoom() + step
                    };
                    inner.apply_picture(inner.current_aspect(), zoom, true);
                }
            ));
            self.actions.add_action(&action);
        }

        // Buttons.
        let osd = &self.osd;
        osd.play.connect_clicked(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    warn(inner.player.toggle_pause());
                }
            }
        ));
        osd.previous.connect_clicked(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.play_neighbour(false);
                }
            }
        ));
        osd.next.connect_clicked(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.play_neighbour(true);
                }
            }
        ));
        osd.fullscreen.connect_clicked(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.toggle_fullscreen();
                }
            }
        ));
        osd.svp.connect_toggled(glib::clone!(
            #[strong]
            weak,
            move |button| {
                let Some(inner) = weak.upgrade() else { return };
                if let Some(session) = inner.session()
                    && session.svp_enabled() != button.is_active()
                {
                    session.set_svp(button.is_active());
                }
                inner.update_svp_status();
            }
        ));
        osd.volume.connect_change_value(glib::clone!(
            #[strong]
            weak,
            move |_, _, value| {
                if let Some(inner) = weak.upgrade() {
                    warn(inner.player.set_volume(value));
                }
                glib::Propagation::Proceed
            }
        ));
        osd.seek.connect_change_value(glib::clone!(
            #[strong]
            weak,
            move |_, _, value| {
                if let Some(inner) = weak.upgrade() {
                    inner.seeked_at.set(Some(Instant::now()));
                    warn(inner.player.seek_absolute(value));
                    inner.preview_at(value, true);
                }
                glib::Propagation::Proceed
            }
        ));
        // Hovering the seek bar previews that point.
        let hover = gtk::EventControllerMotion::new();
        hover.connect_motion(glib::clone!(
            #[strong]
            weak,
            move |_, x, _| {
                if let Some(inner) = weak.upgrade() {
                    let fraction = (x / f64::from(inner.osd.seek.width().max(1))).clamp(0.0, 1.0);
                    if let Some(duration) = inner.player.duration() {
                        inner.preview_at(fraction * duration, false);
                    }
                }
            }
        ));
        hover.connect_leave(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.osd.preview.hide();
                }
            }
        ));
        osd.seek.add_controller(hover);
        osd.skip.connect_clicked(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.skip();
                }
            }
        ));
        osd.up_next_play.connect_clicked(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.play_neighbour(true);
                }
            }
        ));
        osd.up_next_cancel.connect_clicked(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.up_next.set(UpNext::Cancelled);
                    inner.osd.up_next.set_visible(false);
                }
            }
        ));

        // Pointer: show the OSD on movement, double-click for fullscreen.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[strong]
            weak,
            move |_, x, y| {
                let Some(inner) = weak.upgrade() else { return };
                // Wayland repeats motion events on redraw; only real moves count.
                if inner.last_motion.replace(Some((x, y))) != Some((x, y)) {
                    inner.show_osd();
                }
            }
        ));
        overlay.add_controller(motion);
        let clicks = gtk::GestureClick::new();
        clicks.connect_pressed(glib::clone!(
            #[strong]
            weak,
            move |_, presses, _, _| {
                if presses == 2
                    && let Some(inner) = weak.upgrade()
                {
                    inner.toggle_fullscreen();
                }
            }
        ));
        self.video.add_controller(clicks);

        // Keys.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(glib::clone!(
            #[strong]
            weak,
            move |_, key, _, _| match weak.upgrade() {
                Some(inner) => inner.key(key),
                None => glib::Propagation::Proceed,
            }
        ));
        keys.connect_key_released(glib::clone!(
            #[strong]
            weak,
            move |_, key, _, _| {
                if let Some(inner) = weak.upgrade()
                    && inner.key_held.get().is_some_and(|(held, _)| held == key)
                {
                    inner.key_held.set(None);
                }
            }
        ));
        self.page.add_controller(keys);

        // Page lifecycle: tick only while shown; leaving stops playback.
        self.page.connect_shown(glib::clone!(
            #[strong]
            weak,
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.start_tick();
                }
            }
        ));
        self.page.connect_hidden(glib::clone!(
            #[strong]
            weak,
            move |_| {
                let Some(inner) = weak.upgrade() else { return };
                if let Some(tick) = inner.tick.take() {
                    tick.remove();
                }
                inner.set_fullscreen(false);
                inner.page.set_cursor(None::<&gdk::Cursor>);
                inner.session.replace(None);
                if let Some(handlers) = inner.handlers() {
                    (handlers.stopped)();
                }
            }
        ));
    }

    fn session(&self) -> Option<Rc<PlaybackSession>> {
        self.session.borrow().clone()
    }

    fn handlers(&self) -> Option<Rc<Handlers>> {
        self.handlers.borrow().clone()
    }

    fn handle(self: &Rc<Self>, event: PlayerEvent) {
        match event {
            PlayerEvent::Geometry => player::video_area::refit(self.player, &self.video),
            PlayerEvent::Pause(paused) => {
                self.osd.play.set_icon_name(if paused {
                    crate::ui::icons::PLAY
                } else {
                    crate::ui::icons::PAUSE
                });
                self.show_osd();
                if let Some(session) = self.session() {
                    session.report_progress(Some(if paused { "Pause" } else { "Unpause" }));
                }
            }
            PlayerEvent::Duration(_) => self.refresh_duration(),
            // The music queue's business (see `music`).
            PlayerEvent::PlaylistPos(_) => {}
            PlayerEvent::Tracks => self.rebuild_menus(),
            PlayerEvent::Volume => {
                self.sync_volume();
                self.schedule_volume_report();
                self.flash_volume();
            }
            PlayerEvent::FileLoaded => {
                if let Some(session) = self.session() {
                    session.apply_initial_tracks();
                }
                self.refresh_duration();
                self.rebuild_menus();
            }
            PlayerEvent::Finished(END_FILE_REASON_EOF) => self.finished(),
            PlayerEvent::Finished(END_FILE_REASON_ERROR) => {
                if let Some(handlers) = self.handlers() {
                    (handlers.toast)("mpv could not play this stream");
                }
                self.pop();
            }
            // Our own stop/replace.
            PlayerEvent::Finished(_) => {}
        }
    }

    /// End of file: on to the next episode unless the user said no.
    fn finished(self: &Rc<Self>) {
        let has_next = self.session().is_some_and(|s| s.next.is_some());
        if has_next && self.up_next.get() != UpNext::Cancelled {
            self.play_neighbour(true);
        } else {
            self.pop();
        }
    }

    fn play_neighbour(&self, forward: bool) {
        let Some(session) = self.session() else {
            return;
        };
        let target = if forward {
            session.next.clone()
        } else {
            session.previous.clone()
        };
        if let (Some(item), Some(handlers)) = (target, self.handlers()) {
            (handlers.play)(&item);
        }
    }

    fn skip(&self) {
        let (Some(session), Some(position)) = (self.session(), self.player.position()) else {
            return;
        };
        if let Some((_, end)) = session.markers.intro
            && session.markers.in_intro(position)
        {
            warn(self.player.seek_absolute(end));
        } else if session.next.is_some() {
            self.play_neighbour(true);
        } else {
            self.pop();
        }
    }

    fn refresh_duration(&self) {
        let Some(duration) = self.player.duration() else {
            return;
        };
        let seek = &self.osd.seek;
        seek.set_range(0.0, duration);
        seek.clear_marks();
        if let Some(session) = self.session() {
            for &chapter in session.markers.chapters.iter().filter(|&&c| c > 0.0) {
                seek.add_mark(chapter, gtk::PositionType::Bottom, None);
            }
        }
    }

    fn current_aspect(&self) -> Aspect {
        self.actions
            .lookup_action("aspect")
            .and_then(|action| action.state())
            .and_then(|state| state.get::<String>())
            .and_then(|name| Aspect::from_name(&name))
            .unwrap_or_default()
    }

    /// Applies aspect and zoom, refits, and (when `remember`) saves them
    /// for the title.
    fn apply_picture(&self, aspect: Aspect, zoom: f64, remember: bool) {
        warn(self.player.set_aspect(aspect));
        self.player.set_user_zoom(zoom);
        set_state(&self.actions, "aspect", aspect.name().to_variant());
        player::video_area::refit(self.player, &self.video);
        if remember && let Some(session) = self.session() {
            session.remember_picture(aspect, self.player.user_zoom());
        }
    }

    fn rebuild_menus(&self) {
        let session = self.session();
        let entries = |kind| {
            session
                .as_ref()
                .map(|s| s.track_entries(kind))
                .unwrap_or_default()
        };
        let audio = entries(TrackKind::Audio);
        let subtitles = entries(TrackKind::Subtitle);

        self.osd
            .audio
            .set_menu_model(Some(&track_menu("player.audio", &audio, None)));
        self.osd.audio.set_sensitive(audio.len() > 1);
        set_state(
            &self.actions,
            "audio",
            selected(&audio).unwrap_or(0).to_variant(),
        );

        self.osd.subtitles.set_menu_model(Some(&track_menu(
            "player.subtitle",
            &subtitles,
            Some(SUBTITLES_OFF_ID),
        )));
        self.osd.subtitles.set_sensitive(!subtitles.is_empty());
        set_state(
            &self.actions,
            "subtitle",
            selected(&subtitles)
                .unwrap_or(SUBTITLES_OFF_ID)
                .to_variant(),
        );

        let menu = gio::Menu::new();
        for (index, quality) in QUALITIES.iter().enumerate() {
            let item = gio::MenuItem::new(Some(&quality.label()), None);
            item.set_action_and_target_value(
                Some("player.quality"),
                Some(&(index as i32).to_variant()),
            );
            menu.append_item(&item);
        }
        self.osd.quality.set_menu_model(Some(&menu));
        let current = QUALITIES
            .iter()
            .position(|q| *q == self.quality.get())
            .unwrap_or(0);
        set_state(&self.actions, "quality", (current as i32).to_variant());
    }

    fn start_tick(self: &Rc<Self>) {
        if let Some(tick) = self.tick.take() {
            tick.remove();
        }
        let weak = Rc::downgrade(self);
        let tick = glib::timeout_add_local(TICK_INTERVAL, move || match weak.upgrade() {
            Some(inner) => {
                inner.on_tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        self.tick.replace(Some(tick));
    }

    /// Position-driven updates: seek bar, time labels, skip button, Up Next.
    fn on_tick(self: &Rc<Self>) {
        let (Some(position), Some(duration)) = (self.player.position(), self.player.duration())
        else {
            return;
        };
        // Also while scrubbing: the bar shows the target, not the position.
        let dragging = self.scrub_target.get().is_some()
            || self
                .seeked_at
                .get()
                .is_some_and(|at| at.elapsed() < Duration::from_millis(500));
        if !dragging {
            self.osd.seek.set_value(position);
        }
        let ticks = |seconds: f64| (seconds * TICKS_PER_SECOND as f64) as i64;
        self.osd
            .elapsed
            .set_label(&format_timestamp(ticks(position)));
        self.osd.remaining.set_label(&format!(
            "-{}",
            format_timestamp(ticks(duration - position))
        ));

        let Some(session) = self.session() else {
            return;
        };
        if self
            .svp_checked_at
            .get()
            .is_none_or(|at| at.elapsed() >= SVP_CHECK_INTERVAL)
        {
            self.svp_checked_at.set(Some(Instant::now()));
            self.update_svp_status();
        }
        let markers = &session.markers;
        // Music just moves on to the next track; the countdown is for episodes.
        let up_next_due = session.next.is_some()
            && !session.item.is_audio()
            && position >= markers.up_next_at(duration);
        let skip_label = if markers.in_intro(position) {
            Some("Skip Intro")
        } else if markers.in_credits(position) && !up_next_due {
            Some("Skip Credits")
        } else {
            None
        };
        if let Some(label) = skip_label {
            self.osd.skip.set_label(label);
        }
        self.osd.skip.set_visible(skip_label.is_some());

        match (self.up_next.get(), up_next_due) {
            (UpNext::Hidden, true) => {
                self.up_next
                    .set(UpNext::Counting(Instant::now() + UP_NEXT_COUNTDOWN));
                self.osd.up_next.set_visible(true);
            }
            (UpNext::Counting(_), false) => {
                // Seeked back out of the window.
                self.up_next.set(UpNext::Hidden);
                self.osd.up_next.set_visible(false);
            }
            (UpNext::Counting(deadline), true) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    self.play_neighbour(true);
                } else {
                    self.osd
                        .up_next_countdown
                        .set_label(&format!("Playing in {} s", left.as_secs() + 1));
                }
            }
            _ => {}
        }
    }

    /// Colours the SVP button by what's actually happening, and explains
    /// in its tooltip: interpolating, waiting for SVP Manager, or why not.
    fn update_svp_status(&self) {
        let button = &self.osd.svp;
        for class in SVP_STATE_CLASSES {
            button.remove_css_class(class);
        }
        if !crate::svp::installed() {
            button.set_sensitive(false);
            button.set_tooltip_text(Some("SVP isn't installed (~/SVP4 not found)"));
            return;
        }
        button.set_sensitive(true);
        let (class, tooltip) = if !button.is_active() {
            (None, "SVP is off for this title")
        } else if self.player.svp_attached() {
            (Some("svp-active"), "SVP is interpolating")
        } else if crate::svp::manager_running() {
            (Some("svp-waiting"), "Waiting for SVP Manager to attach…")
        } else {
            if !self.svp_warned.replace(true)
                && let Some(handlers) = self.handlers()
            {
                (handlers.toast)("SVP Manager isn't running, so there's no interpolation");
            }
            (Some("svp-missing"), "SVP Manager isn't running")
        };
        if let Some(class) = class {
            button.add_css_class(class);
        }
        button.set_tooltip_text(Some(tooltip));
    }

    /// The OSD's volume slider and icon, from mpv.
    fn sync_volume(&self) {
        self.osd.volume.set_value(self.player.volume());
        self.osd
            .volume_button
            .set_icon_name(if self.player.is_muted() {
                crate::ui::icons::MUTED
            } else {
                crate::ui::icons::VOLUME
            });
    }

    /// Shows the volume pop-up for a moment when volume or mute changed.
    fn flash_volume(self: &Rc<Self>) {
        let (volume, muted) = (self.player.volume(), self.player.is_muted());
        let seen = (volume.round() as i64, muted);
        // The first reading is mpv's starting state, not a change.
        if self
            .volume_seen
            .replace(Some(seen))
            .is_none_or(|last| last == seen)
        {
            return;
        }
        let osd = &self.osd;
        osd.volume_osd_icon.set_icon_name(Some(if muted {
            crate::ui::icons::MUTED
        } else {
            crate::ui::icons::VOLUME
        }));
        osd.volume_osd_level.set_fraction(if muted {
            0.0
        } else {
            (volume / 100.0).clamp(0.0, 1.0)
        });
        osd.volume_osd_label.set_label(&if muted {
            "Muted".to_string()
        } else {
            format!("{}%", seen.0)
        });
        osd.volume_osd.set_reveal_child(true);
        if let Some(pending) = self.volume_osd_timer.take() {
            pending.remove();
        }
        let weak = Rc::downgrade(self);
        let pending = glib::timeout_add_local_once(VOLUME_OSD_TIME, move || {
            if let Some(inner) = weak.upgrade() {
                inner.volume_osd_timer.take();
                inner.osd.volume_osd.set_reveal_child(false);
            }
        });
        self.volume_osd_timer.replace(Some(pending));
    }

    fn schedule_volume_report(self: &Rc<Self>) {
        if let Some(pending) = self.volume_report.take() {
            pending.remove();
        }
        let weak = Rc::downgrade(self);
        let pending = glib::timeout_add_local_once(VOLUME_REPORT_DELAY, move || {
            let Some(inner) = weak.upgrade() else { return };
            inner.volume_report.take();
            if let Some(session) = inner.session() {
                session.report_progress(Some("VolumeChange"));
            }
        });
        self.volume_report.replace(Some(pending));
    }

    /// Shows the scrub preview for `seconds`, over that point of the seek
    /// bar; `flash`: hide it again shortly (no pointer hovering there).
    fn preview_at(&self, seconds: f64, flash: bool) {
        let Some(overlay) = self.page.child() else {
            return;
        };
        let seek = &self.osd.seek;
        let duration = self.player.duration().unwrap_or(0.0);
        if duration <= 0.0 || !self.osd.bottom.reveals_child() {
            return;
        }
        let fraction = (seconds / duration).clamp(0.0, 1.0);
        let x = fraction * f64::from(seek.width());
        let Some(point) = seek.compute_point(&overlay, &gtk::graphene::Point::new(x as f32, 0.0))
        else {
            return;
        };
        let (x, top) = (f64::from(point.x()), f64::from(point.y()));
        if flash {
            self.osd.preview.flash_at(&overlay, seconds, x, top);
        } else {
            self.osd.preview.show_at(&overlay, seconds, x, top);
        }
    }

    /// A relative seek (controller or arrow keys), previewed on the bar.
    /// A press seeks at once. Held for [`HOLD_TO_SCRUB`], it scrubs: the
    /// target moves along the bar with the preview, and mpv seeks there
    /// once nothing has moved it for [`SCRUB_COMMIT`]. Presses while
    /// scrubbing move the target too.
    fn seek_by(self: &Rc<Self>, seconds: i32, repeat: bool) {
        let (Some(position), Some(duration)) = (self.player.position(), self.player.duration())
        else {
            if !repeat {
                warn(self.player.seek_relative(seconds));
            }
            return;
        };
        let now = Instant::now();
        if !repeat {
            self.hold_started.set(Some(now));
        }
        let scrubbing = self.scrub_target.get().is_some();
        if !scrubbing {
            if repeat {
                // Not held long enough yet.
                if self
                    .hold_started
                    .get()
                    .is_none_or(|start| now.duration_since(start) < HOLD_TO_SCRUB)
                {
                    return;
                }
            } else {
                warn(self.player.seek_relative(seconds));
                let target = (position + f64::from(seconds)).clamp(0.0, duration);
                self.preview_at(target, true);
                return;
            }
        }
        let from = self.scrub_target.get().unwrap_or(position);
        let target = (from + f64::from(seconds)).clamp(0.0, duration);
        self.scrub_target.set(Some(target));
        self.osd.seek.set_value(target);
        self.preview_at(target, false);
        if let Some(pending) = self.scrub_timer.take() {
            pending.remove();
        }
        let weak = Rc::downgrade(self);
        let pending = glib::timeout_add_local_once(SCRUB_COMMIT, move || {
            let Some(inner) = weak.upgrade() else { return };
            inner.scrub_timer.take();
            if let Some(target) = inner.scrub_target.take() {
                inner.seeked_at.set(Some(Instant::now()));
                warn(inner.player.seek_absolute(target));
                inner.preview_at(target, true);
            }
        });
        self.scrub_timer.replace(Some(pending));
    }

    /// Whether this press of `key` is auto-repeat: the key hasn't been
    /// released since it last fired. A release lost to a focus change is
    /// covered by the gap check (auto-repeat fires far more often).
    fn key_repeating(&self, key: gdk::Key) -> bool {
        let now = Instant::now();
        let repeating = self
            .key_held
            .get()
            .is_some_and(|(held, last)| held == key && now.duration_since(last) < KEY_REPEAT_GAP);
        self.key_held.set(Some((key, now)));
        repeating
    }

    fn seek_chapter(&self, forward: bool) {
        let (Some(session), Some(position)) = (self.session(), self.player.position()) else {
            return;
        };
        let target = if forward {
            session.markers.next_chapter(position)
        } else {
            session.markers.previous_chapter(position)
        };
        if let Some(target) = target {
            warn(self.player.seek_absolute(target));
            self.preview_at(target, true);
        }
    }

    fn key(self: &Rc<Self>, key: gdk::Key) -> glib::Propagation {
        let player = self.player;
        match key {
            gdk::Key::space | gdk::Key::k => warn(player.toggle_pause()),
            gdk::Key::Left => {
                self.show_osd();
                self.seek_by(-SEEK_STEP, self.key_repeating(key));
            }
            gdk::Key::Right => {
                self.show_osd();
                self.seek_by(SEEK_STEP, self.key_repeating(key));
            }
            gdk::Key::Up => warn(player.set_volume(player.volume() + VOLUME_STEP)),
            gdk::Key::Down => warn(player.set_volume(player.volume() - VOLUME_STEP)),
            gdk::Key::m => warn(player.toggle_mute()),
            gdk::Key::n | gdk::Key::N => self.play_neighbour(true),
            gdk::Key::p | gdk::Key::P => self.play_neighbour(false),
            gdk::Key::Page_Down => self.seek_chapter(true),
            gdk::Key::Page_Up => self.seek_chapter(false),
            gdk::Key::F11 | gdk::Key::f => self.toggle_fullscreen(),
            gdk::Key::Escape => self.leave(),
            _ => return glib::Propagation::Proceed,
        }
        self.show_osd();
        glib::Propagation::Stop
    }

    /// Reveals the OSD and cursor, then hides both after a quiet spell
    /// (unless paused or a menu is open).
    fn show_osd(self: &Rc<Self>) {
        self.osd.top.set_reveal_child(true);
        self.osd.bottom.set_reveal_child(true);
        self.page.set_cursor(None::<&gdk::Cursor>);
        if let Some(timer) = self.hide_timer.take() {
            timer.remove();
        }
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(OSD_TIMEOUT, move || {
            let Some(inner) = weak.upgrade() else { return };
            inner.hide_timer.take();
            if !inner.page.is_mapped() {
                return;
            }
            if inner.player.is_paused() || inner.osd.menu_open() || inner.controls_mode.get() {
                inner.show_osd();
                return;
            }
            inner.osd.top.set_reveal_child(false);
            inner.osd.bottom.set_reveal_child(false);
            inner.page.set_cursor_from_name(Some("none"));
        });
        self.hide_timer.replace(Some(timer));
    }

    fn window(&self) -> Option<gtk::Window> {
        self.page.root().and_downcast::<gtk::Window>()
    }

    fn toggle_fullscreen(&self) {
        let fullscreen = self.window().is_some_and(|window| window.is_fullscreen());
        self.set_fullscreen(!fullscreen);
    }

    fn set_fullscreen(&self, fullscreen: bool) {
        // In gamescope the whole app stays fullscreen; never shrink it.
        if let Some(window) = self.window()
            && !super::window::fullscreen_locked()
        {
            window.set_fullscreened(fullscreen);
        }
        self.osd.fullscreen.set_icon_name(if fullscreen {
            crate::ui::icons::UNFULLSCREEN
        } else {
            crate::ui::icons::FULLSCREEN
        });
    }

    /// Esc: leave fullscreen first, then the page.
    fn leave(&self) {
        if self.window().is_some_and(|window| window.is_fullscreen())
            && !super::window::fullscreen_locked()
        {
            self.set_fullscreen(false);
        } else {
            self.pop();
        }
    }

    fn pop(&self) {
        if let Some(nav) = self
            .page
            .ancestor(adw::NavigationView::static_type())
            .and_downcast::<adw::NavigationView>()
            && nav.visible_page().as_ref() == Some(&self.page)
        {
            nav.pop();
        }
    }
}

/// OSD title lines: series over "S1:E4 · Name" for episodes.
fn titles(item: &BaseItem) -> (String, String) {
    if item.is_audio() {
        let by = [item.artist(), item.album.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        return (item.name.clone(), by);
    }
    match &item.series_name {
        Some(series) if item.item_type == "Episode" => (series.clone(), item.episode_label()),
        _ => (
            item.name.clone(),
            item.production_year
                .map(|y| y.to_string())
                .unwrap_or_default(),
        ),
    }
}

fn picture_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let bars = gio::Menu::new();
    for (label, name) in [
        ("Off", "off"),
        ("Blurred picture", "blur"),
        ("Edge glow", "glow"),
    ] {
        let item = gio::MenuItem::new(Some(label), None);
        item.set_action_and_target_value(Some("player.bar-fill"), Some(&name.to_variant()));
        bars.append_item(&item);
    }
    menu.append_section(Some("Black bars"), &bars);
    let aspects = gio::Menu::new();
    for aspect in Aspect::ALL {
        let item = gio::MenuItem::new(Some(aspect.label()), None);
        item.set_action_and_target_value(Some("player.aspect"), Some(&aspect.name().to_variant()));
        aspects.append_item(&item);
    }
    menu.append_section(Some("Aspect"), &aspects);
    let zoom = gio::Menu::new();
    zoom.append(Some("Zoom In"), Some("player.zoom-in"));
    zoom.append(Some("Zoom Out"), Some("player.zoom-out"));
    zoom.append(Some("Reset Zoom"), Some("player.zoom-reset"));
    menu.append_section(Some("Zoom"), &zoom);
    menu
}

pub fn bar_fill_from_name(name: &str) -> BarFill {
    match name {
        "blur" => BarFill::Blur,
        "glow" => BarFill::Glow,
        _ => BarFill::Off,
    }
}

fn track_menu(action: &str, entries: &[TrackEntry], off: Option<i64>) -> gio::Menu {
    let menu = gio::Menu::new();
    if let Some(off) = off {
        let item = gio::MenuItem::new(Some("Off"), None);
        item.set_action_and_target_value(Some(action), Some(&off.to_variant()));
        menu.append_item(&item);
    }
    for entry in entries {
        let item = gio::MenuItem::new(Some(&entry.label), None);
        item.set_action_and_target_value(Some(action), Some(&entry.id.to_variant()));
        menu.append_item(&item);
    }
    menu
}

fn selected(entries: &[TrackEntry]) -> Option<i64> {
    entries.iter().find(|e| e.selected).map(|e| e.id)
}

/// Updates a radio action's state without activating it.
fn set_state(actions: &gio::SimpleActionGroup, name: &str, value: glib::Variant) {
    if let Some(action) = actions
        .lookup_action(name)
        .and_downcast::<gio::SimpleAction>()
    {
        action.set_state(&value);
    }
}

fn warn(result: anyhow::Result<()>) {
    if let Err(e) = result {
        tracing::warn!("{e:#}");
    }
}
