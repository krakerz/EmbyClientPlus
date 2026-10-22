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
use crate::playback::{Origin, PlaybackSession, QUALITIES, Quality, TICKS_PER_SECOND, TrackEntry};
use crate::player::compositor::BarFill;
use crate::player::{
    self, Aspect, END_FILE_REASON_EOF, END_FILE_REASON_ERROR, Player, PlayerEvent, TrackKind,
};

/// OSD hides after this long without pointer movement while playing.
const OSD_TIMEOUT: Duration = Duration::from_secs(3);
/// Position/markers/Up Next refresh rate while the page is shown.
const TICK_INTERVAL: Duration = Duration::from_millis(250);
const SEEK_STEP: i32 = 10;
/// One zoom step, in mpv's log2 units (about 7%).
const ZOOM_STEP: f64 = 0.1;
/// How often the SVP button re-checks whether SVP attached.
const SVP_CHECK_INTERVAL: Duration = Duration::from_secs(2);
/// Holding an arrow key this long starts scrubbing (a tap seeks right away).
const HOLD_TO_SCRUB: Duration = Duration::from_secs(1);
/// How often a tilted right stick moves the scrub target.
const STICK_TICK: Duration = Duration::from_millis(33);
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
    Counting,
    /// The user dismissed it; don't autoplay this item's successor.
    Cancelled,
}

/// What the sleep menu offers.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SleepChoice {
    Off,
    After(u32),
    EndOfVideo,
}

const SLEEP_CHOICES: [SleepChoice; 7] = [
    SleepChoice::Off,
    SleepChoice::After(15),
    SleepChoice::After(30),
    SleepChoice::After(45),
    SleepChoice::After(60),
    SleepChoice::After(90),
    SleepChoice::EndOfVideo,
];

impl SleepChoice {
    /// The action's target string.
    fn name(self) -> String {
        match self {
            SleepChoice::Off => "off".into(),
            SleepChoice::After(minutes) => minutes.to_string(),
            SleepChoice::EndOfVideo => "end".into(),
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        SLEEP_CHOICES.into_iter().find(|c| c.name() == name)
    }

    fn label(self, episode: bool) -> String {
        match self {
            SleepChoice::Off => "Off".into(),
            SleepChoice::After(60) => "1 hour".into(),
            SleepChoice::After(minutes) => format!("{minutes} minutes"),
            SleepChoice::EndOfVideo if episode => "End of this episode".into(),
            SleepChoice::EndOfVideo => "End of this video".into(),
        }
    }
}

/// The sleep timer's running state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Sleep {
    Off,
    /// Leave the page at this instant.
    At(Instant),
    /// Leave when the current video (and any after a manual Next) ends.
    EndOfVideo,
}

/// Time left on the sleep timer, rounded up to whole minutes ("23m", "1h05m").
fn sleep_remaining(left: Duration) -> String {
    let minutes = left.as_secs().div_ceil(60).max(1);
    if minutes >= 60 {
        format!("{}h{:02}m", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}m")
    }
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
    /// Preferences' Up Next countdown, read when playback starts.
    up_next_seconds: Cell<f64>,
    /// The account's (or player-only mode's) autoplay and intro skipping,
    /// read when playback starts.
    autoplay_next: Cell<bool>,
    intro_skip: Cell<crate::config::IntroSkip>,
    /// The intro was skipped automatically already (seeking back into it
    /// is then the user's choice).
    intro_auto_skipped: Cell<bool>,
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
    /// Right-stick tilt (-1.0..=1.0) and the timer moving the scrub target
    /// while it's tilted.
    stick_tilt: Cell<f32>,
    stick_timer: RefCell<Option<glib::SourceId>>,
    volume_osd_timer: RefCell<Option<glib::SourceId>>,
    svp_checked_at: Cell<Option<Instant>>,
    /// The "SVP Manager isn't running" toast shows once per item.
    svp_warned: Cell<bool>,
    /// Held while a video plays, so the screen stays on.
    screen_inhibit: RefCell<Option<(gtk::Application, u32)>>,
    /// Kept across autoplay/Next; cleared when the page is left.
    sleep: Cell<Sleep>,
    /// Fires the timed sleep. Taken (not removed) when it fires.
    sleep_timer: RefCell<Option<glib::SourceId>>,
}

struct Osd {
    top: gtk::Revealer,
    /// Top right: where the video comes from, when it isn't streamed.
    origin: gtk::Box,
    origin_icon: gtk::Image,
    origin_label: gtk::Label,
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
    shaders: gtk::MenuButton,
    /// Moon icon, plus the time left (or "End") while a sleep timer runs.
    sleep: gtk::MenuButton,
    sleep_label: gtk::Label,
    svp: gtk::ToggleButton,
    fullscreen: gtk::Button,
    mute: gtk::Button,
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
            up_next_seconds: Cell::new(10.0),
            autoplay_next: Cell::new(true),
            intro_skip: Cell::new(crate::config::IntroSkip::default()),
            intro_auto_skipped: Cell::new(false),
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
            stick_tilt: Cell::new(0.0),
            stick_timer: RefCell::new(None),
            volume_osd_timer: RefCell::new(None),
            svp_checked_at: Cell::new(None),
            svp_warned: Cell::new(false),
            screen_inhibit: RefCell::new(None),
            sleep: Cell::new(Sleep::Off),
            sleep_timer: RefCell::new(None),
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
                inner.seek_by(-SEEK_STEP, repeat, false);
            }
            Action::SeekForward => {
                inner.show_osd();
                inner.seek_by(SEEK_STEP, repeat, false);
            }
            // Volume shows only its own indicator (`flash_volume`), not the
            // whole OSD.
            Action::VolumeUp => {
                return warn(player.set_volume(player.volume() + VOLUME_STEP));
            }
            Action::VolumeDown => {
                return warn(player.set_volume(player.volume() - VOLUME_STEP));
            }
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

    /// The right stick's tilt (0.0: let go), for scrubbing.
    pub fn scrub_stick(&self, tilt: f32) {
        self.inner.scrub_stick(tilt);
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
        self.inner.stop_stick();
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
        let prefs = crate::account::playback();
        inner.autoplay_next.set(prefs.autoplay_next);
        inner.intro_skip.set(prefs.intro_skip);
        inner.intro_auto_skipped.set(false);
        let playback = crate::config::Settings::load().unwrap_or_default().playback;
        inner
            .up_next_seconds
            .set(f64::from(playback.up_next_seconds.clamp(3, 60)));
        inner
            .osd
            .up_next
            .set_opacity(playback.up_next_opacity.clamp(0.2, 1.0));
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
    /// `client`: the Emby server it plays from; `None` for local files.
    pub fn attach(
        &self,
        session: Rc<PlaybackSession>,
        client: Option<&std::sync::Arc<EmbyClient>>,
    ) {
        let inner = &self.inner;
        if let Some(client) = client {
            inner
                .osd
                .preview
                .load(client, &session.item, session.trickplay_source());
        }
        // Nothing to transcode without a server; say where it plays from.
        inner.osd.quality.set_visible(
            session.is_from_server()
                || (session.origin == Origin::Download
                    && crate::config::Settings::load()
                        .unwrap_or_default()
                        .playback
                        .download_quality_streams),
        );
        let origin = match session.origin {
            Origin::Server | Origin::Link => None,
            Origin::Download => Some(("Downloaded", "Playing the downloaded copy on this device")),
            Origin::File => Some(("Local file", "Playing a file on this device")),
        };
        inner.osd.origin.set_visible(origin.is_some());
        if let Some((label, tooltip)) = origin {
            inner
                .osd
                .origin_icon
                .set_icon_name(Some(match session.origin {
                    Origin::Download => crate::ui::icons::DOWNLOAD,
                    _ => crate::ui::icons::FOLDER,
                }));
            inner.osd.origin_label.set_label(label);
            inner.osd.origin.set_tooltip_text(Some(tooltip));
        }
        inner.osd.previous.set_sensitive(session.previous.is_some());
        inner.osd.next.set_sensitive(session.next.is_some());
        inner
            .osd
            .sleep
            .set_menu_model(Some(&sleep_menu(session.item.item_type == "Episode")));
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
            if let Some(client) = client {
                images::load_with_client(client, &inner.osd.up_next_picture, next.landscape(), 384);
            }
        }
        let (aspect, zoom) = session.picture();
        inner.session.replace(Some(session));
        inner.apply_picture(aspect, zoom, false);
        inner.refresh_duration();
        inner.rebuild_menus();
    }
}

impl Osd {
    /// Tooltips name the key that does the same, as currently mapped
    /// (Preferences → Keyboard); with the controller in use, just the name.
    fn refresh_hints(&self) {
        use crate::keys::KeyAction;
        let bindings = crate::keys::bindings();
        let pad = crate::ui::legend::pad_in_use();
        let buttons: [(&gtk::Button, &str, KeyAction); 5] = [
            (
                &self.previous,
                "Previous episode",
                KeyAction::PreviousEpisode,
            ),
            (&self.play, "Play/Pause", KeyAction::PlayPause),
            (&self.next, "Next episode", KeyAction::NextEpisode),
            (&self.mute, "Mute", KeyAction::Mute),
            (&self.fullscreen, "Fullscreen", KeyAction::Fullscreen),
        ];
        for (button, name, action) in buttons {
            let key = bindings.keys(action).into_iter().next().filter(|_| !pad);
            let text = match key {
                Some(key) => format!("{name} ({})", crate::keys::key_label(&key)),
                None => name.to_string(),
            };
            button.set_tooltip_text(Some(&text));
        }
    }

    fn build(video: &gtk::GLArea) -> (Osd, gtk::Overlay) {
        let icon_button = |icon: &str, tooltip: &str| {
            gtk::Button::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .css_classes(["flat", "circular", "osd-button"])
                // Never stretched by a taller neighbour: stays a circle.
                .valign(gtk::Align::Center)
                .build()
        };
        let menu_button = |icon: &str, tooltip: &str| {
            gtk::MenuButton::builder()
                .icon_name(icon)
                .tooltip_text(tooltip)
                .direction(gtk::ArrowType::Up)
                .css_classes(["flat", "circular", "osd-button"])
                .valign(gtk::Align::Center)
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
        let titles = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();
        titles.append(&title);
        titles.append(&subtitle);
        let origin_icon = gtk::Image::new();
        let origin_label = gtk::Label::new(None);
        let origin = gtk::Box::builder()
            .spacing(6)
            .valign(gtk::Align::Center)
            .css_classes(["origin-badge"])
            .visible(false)
            .build();
        origin.append(&origin_icon);
        origin.append(&origin_label);
        let top_bar = gtk::Box::builder()
            .spacing(12)
            .css_classes(["osd", "player-bar"])
            .build();
        top_bar.append(&back);
        top_bar.append(&titles);
        top_bar.append(&origin);
        // The player hides the window's title bar, so this bar stands in for
        // it: drag to move the window, double-click to maximize.
        let handle = gtk::WindowHandle::builder().child(&top_bar).build();
        let top = gtk::Revealer::builder()
            .child(&handle)
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

        let previous = icon_button(crate::ui::icons::SKIP_BACK, "Previous episode");
        let play = icon_button(crate::ui::icons::PAUSE, "Play/Pause");
        play.add_css_class("large-button");
        let next = icon_button(crate::ui::icons::SKIP_FORWARD, "Next episode");
        let volume = gtk::Scale::builder()
            .orientation(gtk::Orientation::Horizontal)
            .width_request(180)
            .draw_value(false)
            .build();
        volume.set_range(0.0, 130.0);
        volume.add_mark(100.0, gtk::PositionType::Bottom, None);
        let mute = gtk::Button::builder()
            .icon_name(crate::ui::icons::MUTED)
            .tooltip_text("Mute")
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
        let shaders = menu_button(crate::ui::icons::SHADERS, "Shaders");
        let sleep = menu_button(crate::ui::icons::SLEEP, "Sleep timer");
        sleep.set_menu_model(Some(&sleep_menu(true)));
        let sleep_label = gtk::Label::builder()
            .css_classes(["numeric", "caption-heading"])
            .visible(false)
            .build();
        let sleep_content = gtk::Box::builder()
            .spacing(4)
            .halign(gtk::Align::Center)
            .build();
        sleep_content.append(&gtk::Image::from_icon_name(crate::ui::icons::SLEEP));
        sleep_content.append(&sleep_label);
        sleep.set_child(Some(&sleep_content));
        let svp = gtk::ToggleButton::builder()
            // An icon, not a label: text carries the font's line spacing and
            // never sits quite centred in the circle.
            .child(
                &gtk::Image::builder()
                    .icon_name(crate::ui::icons::SVP)
                    .pixel_size(24)
                    .build(),
            )
            .tooltip_text("Frame interpolation through SVP (remembered for this title)")
            .css_classes(["flat", "svp-toggle", "osd-button"])
            .valign(gtk::Align::Center)
            .build();
        let fullscreen = icon_button(crate::ui::icons::FULLSCREEN, "Fullscreen");

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
        end.append(&shaders);
        end.append(&sleep);
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
            origin,
            origin_icon,
            origin_label,
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
            shaders,
            sleep,
            sleep_label,
            svp,
            fullscreen,
            mute,
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
            &self.shaders,
            &self.sleep,
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
        let sleep = gio::SimpleAction::new_stateful(
            "sleep",
            Some(glib::VariantTy::STRING),
            &SleepChoice::Off.name().to_variant(),
        );
        sleep.connect_activate(glib::clone!(
            #[strong]
            weak,
            move |action, value| {
                let (Some(inner), Some(choice)) = (
                    weak.upgrade(),
                    value
                        .and_then(|v| v.get::<String>())
                        .and_then(|name| SleepChoice::from_name(&name)),
                ) else {
                    return;
                };
                action.set_state(&choice.name().to_variant());
                inner.set_sleep(choice);
            }
        ));
        self.actions.add_action(&sleep);

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
            move |_, key, _, modifiers| {
                // Ctrl/⌘/Alt combinations are app shortcuts (⌘, Settings,
                // ⌘Q Quit), not the player's single keys.
                let held = gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::META_MASK
                    | gdk::ModifierType::SUPER_MASK
                    | gdk::ModifierType::ALT_MASK;
                crate::ui::legend::keyboard_used();
                match weak.upgrade() {
                    Some(inner) if !modifiers.intersects(held) => inner.key(key),
                    _ => glib::Propagation::Proceed,
                }
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
        // On the window, not the page: key events only travel the widgets on
        // the way to the focus, and after a click elsewhere (or a page
        // change) the focus can be outside the player, or on nothing.
        self.page.connect_realize(move |page| {
            if keys.widget().is_none()
                && let Some(root) = page.root()
            {
                root.add_controller(keys.clone());
            }
        });

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
                inner.stop_stick();
                inner.set_sleep(SleepChoice::Off);
                set_state(
                    &inner.actions,
                    "sleep",
                    SleepChoice::Off.name().to_variant(),
                );
                inner.set_fullscreen(false);
                inner.keep_screen_on(false);
                #[cfg(target_os = "linux")]
                crate::dnd::set(false);
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
                self.keep_screen_on(!paused);
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
                self.keep_screen_on(!self.player.is_paused());
                // Paused or not: notifications wait until the player closes.
                #[cfg(target_os = "linux")]
                crate::dnd::set(self.session().is_some_and(|s| !s.item.is_audio()));
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
        // A stale EOF from the previous file, while the next one loads.
        let Some(session) = self.session() else {
            return;
        };
        session.mark_ended();
        let has_next = session.next.is_some();
        if self.sleep.get() == Sleep::EndOfVideo {
            self.sleep_expired();
        } else if has_next && self.autoplay_next.get() && self.up_next.get() != UpNext::Cancelled {
            self.play_neighbour(true);
        } else {
            self.pop();
        }
    }

    /// Sets (or, with Off, clears) the sleep timer. A timed one counts from
    /// now, in wall-clock time, whatever the video does meanwhile.
    fn set_sleep(self: &Rc<Self>, choice: SleepChoice) {
        if let Some(timer) = self.sleep_timer.take() {
            timer.remove();
        }
        self.sleep.set(match choice {
            SleepChoice::Off => Sleep::Off,
            SleepChoice::EndOfVideo => Sleep::EndOfVideo,
            SleepChoice::After(minutes) => {
                let after = Duration::from_secs(u64::from(minutes) * 60);
                let weak = Rc::downgrade(self);
                let timer = glib::timeout_add_local_once(after, move || {
                    if let Some(inner) = weak.upgrade() {
                        // Fired: the id is spent, removing it would warn.
                        inner.sleep_timer.take();
                        inner.sleep_expired();
                    }
                });
                self.sleep_timer.replace(Some(timer));
                Sleep::At(Instant::now() + after)
            }
        });
        self.refresh_sleep();
    }

    /// Time's up: leave like Back, so the stop is reported and the screen
    /// can sleep.
    fn sleep_expired(&self) {
        self.sleep.set(Sleep::Off);
        if let Some(handlers) = self.handlers() {
            (handlers.toast)("Sleep timer: playback stopped");
        }
        self.pop();
        // Game Mode never idles into sleep on its own; the delay lets the
        // stop report reach the server first.
        #[cfg(target_os = "linux")]
        if crate::gamescope::detected() {
            glib::timeout_add_seconds_local_once(5, crate::gamescope::suspend);
        }
    }

    /// The sleep button's label and tooltip; cheap enough for every tick.
    fn refresh_sleep(&self) {
        let (label, tooltip) = match self.sleep.get() {
            Sleep::Off => (None, "Sleep timer".to_string()),
            Sleep::EndOfVideo => (Some("End".to_string()), "Sleep at the end".to_string()),
            Sleep::At(at) => {
                let left = sleep_remaining(at.saturating_duration_since(Instant::now()));
                (Some(left.clone()), format!("Sleep in {left}"))
            }
        };
        let osd = &self.osd;
        osd.sleep_label.set_visible(label.is_some());
        if label.is_some() {
            osd.sleep.add_css_class("with-label");
        } else {
            osd.sleep.remove_css_class("with-label");
        }
        osd.sleep_label.set_label(label.as_deref().unwrap_or(""));
        osd.sleep.set_tooltip_text(Some(&tooltip));
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

        let subtitle_menu = track_menu("player.subtitle", &subtitles, Some(SUBTITLES_OFF_ID));
        let adjust = gio::Menu::new();
        for id in [SUB_SIZE_ROW, SUB_TIMING_ROW] {
            let item = gio::MenuItem::new(None, None);
            item.set_attribute_value("custom", Some(&id.to_variant()));
            adjust.append_item(&item);
        }
        subtitle_menu.append_section(None, &adjust);
        self.osd.subtitles.set_menu_model(Some(&subtitle_menu));
        if let Some(popover) = self
            .osd
            .subtitles
            .popover()
            .and_downcast::<gtk::PopoverMenu>()
        {
            popover.add_child(&subtitle_size_row(self.player), SUB_SIZE_ROW);
            popover.add_child(&subtitle_timing_row(self.player), SUB_TIMING_ROW);
        }
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
        self.rebuild_shader_menu();
    }

    /// The Shaders menu: one row per group shown (Preferences), checked
    /// while it has a preset on, each opening its list (Off and presets).
    /// Hand-built: menu models can't put a check before a submenu's arrow.
    fn rebuild_shader_menu(&self) {
        let groups = self.session().map(|s| s.shaders()).unwrap_or_default();
        self.osd.shaders.set_visible(!groups.is_empty());
        show_shaders_on(
            &self.osd.shaders,
            groups.iter().any(|(_, preset)| preset.is_some()),
        );
        let popover = gtk::Popover::builder().css_classes(["menu"]).build();
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::SlideLeftRight)
            .vhomogeneous(false)
            .hhomogeneous(false)
            .interpolate_size(true)
            .build();
        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stack.add_named(&main, Some("groups"));
        // Each group's pick, shared by its row and its page.
        let picks: Rc<Vec<RefCell<Option<String>>>> = Rc::new(
            groups
                .iter()
                .map(|(_, preset)| RefCell::new(preset.clone()))
                .collect(),
        );
        let session = self.session().map(|s| Rc::downgrade(&s));
        let button = self.osd.shaders.downgrade();
        for (index, (group, _)) in groups.into_iter().enumerate() {
            let page_name = format!("g{index}");
            let check = check_icon();
            check.set_opacity(if picks[index].borrow().is_some() {
                1.0
            } else {
                0.0
            });
            let row = menu_row(
                &group.name,
                None,
                Some(&check),
                Some(crate::ui::icons::NEXT),
            );
            main.append(&row);

            let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let back = menu_row(
                &group.name,
                Some(&gtk::Image::from_icon_name(crate::ui::icons::BACK)),
                None,
                None,
            );
            back.add_css_class("heading");
            page.append(&back);
            page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            let mut options: Vec<(String, String)> = vec![(String::new(), "Off".into())];
            options.extend(group.presets.iter().map(|p| (p.id.clone(), p.name.clone())));
            let marks: Vec<gtk::Image> = options.iter().map(|_| check_icon()).collect();
            let show_marks = {
                let (marks, ids) = (
                    marks.clone(),
                    options.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
                );
                move |pick: &Option<String>| {
                    let current = pick.clone().unwrap_or_default();
                    for (mark, id) in marks.iter().zip(&ids) {
                        mark.set_opacity(if *id == current { 1.0 } else { 0.0 });
                    }
                }
            };
            show_marks(&picks[index].borrow());
            for ((id, label), mark) in options.into_iter().zip(marks) {
                let option = menu_row(&label, Some(&mark), None, None);
                let (session, group_id, picks, check, show_marks, button, popover) = (
                    session.clone(),
                    group.id.clone(),
                    picks.clone(),
                    check.clone(),
                    show_marks.clone(),
                    button.clone(),
                    popover.downgrade(),
                );
                option.connect_clicked(move |_| {
                    let Some(session) = session.as_ref().and_then(Weak::upgrade) else {
                        return;
                    };
                    session.set_shader(&group_id, &id);
                    let pick = (!id.is_empty()).then(|| id.clone());
                    check.set_opacity(if pick.is_some() { 1.0 } else { 0.0 });
                    show_marks(&pick);
                    picks[index].replace(pick);
                    if let Some(button) = button.upgrade() {
                        show_shaders_on(&button, picks.iter().any(|p| p.borrow().is_some()));
                    }
                    if let Some(popover) = popover.upgrade() {
                        popover.popdown();
                    }
                });
                page.append(&option);
            }
            stack.add_named(&page, Some(&page_name));

            let to_page = (stack.downgrade(), page.downgrade());
            row.connect_clicked(move |_| {
                if let (Some(stack), Some(page)) = (to_page.0.upgrade(), to_page.1.upgrade()) {
                    stack.set_visible_child(&page);
                    // On the group's current pick, else its first entry.
                    focus_checked(&page);
                }
            });
            let to_main = (stack.downgrade(), main.downgrade());
            back.connect_clicked(move |_| {
                if let (Some(stack), Some(main)) = (to_main.0.upgrade(), to_main.1.upgrade()) {
                    stack.set_visible_child(&main);
                    main.child_focus(gtk::DirectionType::TabForward);
                }
            });
        }
        popover.set_child(Some(&stack));
        let main_ref = main.downgrade();
        popover.connect_closed(glib::clone!(
            #[weak]
            stack,
            move |_| {
                if let Some(main) = main_ref.upgrade() {
                    stack.set_visible_child(&main);
                }
            }
        ));
        self.osd.shaders.set_popover(Some(&popover));
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
        if matches!(self.sleep.get(), Sleep::At(_)) {
            self.refresh_sleep();
        }
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
        // It runs to the file's very end (scenes often follow the credits),
        // where `finished` moves on; appearing a second early lets the full
        // count show.
        let left_in_file = duration - position;
        let up_next_due = session.next.is_some()
            && self.autoplay_next.get()
            // The video ending is the sleep timer's cue, not an episode change.
            && self.sleep.get() != Sleep::EndOfVideo
            && !session.item.is_audio()
            && duration > 0.0
            && left_in_file <= self.up_next_seconds.get() + 1.0;
        let in_intro = markers.in_intro(position);
        let intro_skip = self.intro_skip.get();
        if in_intro
            && intro_skip == crate::config::IntroSkip::AutoSkip
            && !self.intro_auto_skipped.replace(true)
            && let Some((_, end)) = markers.intro
        {
            warn(self.player.seek_absolute(end));
        }
        let skip_label = if in_intro && intro_skip == crate::config::IntroSkip::ShowButton {
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
                self.up_next.set(UpNext::Counting);
                self.osd.up_next.set_visible(true);
            }
            (UpNext::Counting, false) => {
                // Seeked back out of the window.
                self.up_next.set(UpNext::Hidden);
                self.osd.up_next.set_visible(false);
            }
            (UpNext::Counting, true) => {
                // Follows the video, so pausing holds the count too.
                let left = left_in_file.min(self.up_next_seconds.get()).ceil().max(0.0);
                self.osd
                    .up_next_countdown
                    .set_label(&format!("Playing in {left} s"));
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
            let looked_in = crate::svp::install_dir()
                .map(|dir| dir.display().to_string())
                .unwrap_or_else(|| crate::svp::default_dir_label().to_string());
            button.set_tooltip_text(Some(&format!(
                "SVP isn't installed ({looked_in} not found; set its folder in Preferences → General)"
            )));
            return;
        }
        button.set_sensitive(true);
        let (class, tooltip) = if !button.is_active() {
            (None, "SVP is off for this title")
        } else if let Some(hint) = crate::svp::vapoursynth_hint() {
            if !self.svp_warned.replace(true)
                && let Some(handlers) = self.handlers()
            {
                (handlers.toast)(hint);
            }
            (Some("svp-missing"), hint)
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
                session.remember_volume();
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
    /// A press seeks at once. With `hold_scrubs` (arrow keys), holding it
    /// for [`HOLD_TO_SCRUB`] scrubs: the target moves along the bar with the
    /// preview, and mpv seeks there once nothing has moved it for
    /// [`SCRUB_COMMIT`]; presses while scrubbing move the target too. The
    /// controller scrubs with the right stick instead, so its presses always
    /// seek right away (one waiting to land lands with the press).
    fn seek_by(self: &Rc<Self>, seconds: i32, repeat: bool, hold_scrubs: bool) {
        let (Some(position), Some(duration)) = (self.player.position(), self.player.duration())
        else {
            if !repeat {
                warn(self.player.seek_relative(seconds));
            }
            return;
        };
        if !hold_scrubs {
            if repeat {
                return;
            }
            match self.scrub_target.take() {
                Some(from) => {
                    if let Some(pending) = self.scrub_timer.take() {
                        pending.remove();
                    }
                    let target = (from + f64::from(seconds)).clamp(0.0, duration);
                    self.seeked_at.set(Some(Instant::now()));
                    warn(self.player.seek_absolute(target));
                    self.preview_at(target, true);
                }
                None => {
                    warn(self.player.seek_relative(seconds));
                    let target = (position + f64::from(seconds)).clamp(0.0, duration);
                    self.preview_at(target, true);
                }
            }
            return;
        }
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
        self.land_scrub_later();
    }

    /// Seeks to the scrub target once [`SCRUB_COMMIT`] passes with no move
    /// (each call restarts the wait).
    fn land_scrub_later(self: &Rc<Self>) {
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

    /// Right-stick scrubbing: while it's tilted the target moves along the
    /// bar, faster the further it's pushed, and mpv seeks there
    /// [`SCRUB_COMMIT`] after it's let go (tilting again in that time
    /// carries on from the target).
    fn scrub_stick(self: &Rc<Self>, tilt: f32) {
        self.stick_tilt.set(tilt);
        if tilt == 0.0 {
            if let Some(timer) = self.stick_timer.take() {
                timer.remove();
                if self.scrub_target.get().is_some() {
                    self.land_scrub_later();
                }
            }
            return;
        }
        if self.stick_timer.borrow().is_some() || self.player.duration().is_none() {
            return;
        }
        // Moving again: it lands after the next let-go.
        if let Some(pending) = self.scrub_timer.take() {
            pending.remove();
        }
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local(STICK_TICK, move || {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            inner.stick_step();
            glib::ControlFlow::Continue
        });
        self.stick_timer.replace(Some(timer));
        self.stick_step();
    }

    fn stick_step(self: &Rc<Self>) {
        let (Some(position), Some(duration)) = (self.player.position(), self.player.duration())
        else {
            return;
        };
        let rate = scrub_rate(f64::from(self.stick_tilt.get()), duration);
        let from = self.scrub_target.get().unwrap_or(position);
        let target = (from + rate * STICK_TICK.as_secs_f64()).clamp(0.0, duration);
        self.scrub_target.set(Some(target));
        self.osd.seek.set_value(target);
        self.show_osd();
        self.preview_at(target, false);
    }

    /// Drops a stick scrub in progress (a new video, the player left).
    fn stop_stick(&self) {
        self.stick_tilt.set(0.0);
        if let Some(timer) = self.stick_timer.take() {
            timer.remove();
        }
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
        // Only while the player is what's on screen (not under a dialog).
        let dialog_open = self
            .page
            .root()
            .and_downcast::<adw::ApplicationWindow>()
            .is_some_and(|window| window.visible_dialog().is_some());
        if !self.page.is_mapped() || dialog_open {
            return glib::Propagation::Proceed;
        }
        let player = self.player;
        let Some(action) =
            crate::keys::key_name(key).and_then(|name| crate::keys::bindings().action_for(&name))
        else {
            return glib::Propagation::Proceed;
        };
        use crate::keys::KeyAction;
        match action {
            KeyAction::PlayPause => warn(player.toggle_pause()),
            KeyAction::SeekBack => {
                self.show_osd();
                self.seek_by(-SEEK_STEP, self.key_repeating(key), true);
            }
            KeyAction::SeekForward => {
                self.show_osd();
                self.seek_by(SEEK_STEP, self.key_repeating(key), true);
            }
            // Volume shows only its own indicator (`flash_volume`).
            KeyAction::VolumeUp => {
                warn(player.set_volume(player.volume() + VOLUME_STEP));
                return glib::Propagation::Stop;
            }
            KeyAction::VolumeDown => {
                warn(player.set_volume(player.volume() - VOLUME_STEP));
                return glib::Propagation::Stop;
            }
            KeyAction::Mute => {
                warn(player.toggle_mute());
                return glib::Propagation::Stop;
            }
            KeyAction::NextEpisode => self.play_neighbour(true),
            KeyAction::PreviousEpisode => self.play_neighbour(false),
            KeyAction::NextChapter => self.seek_chapter(true),
            KeyAction::PreviousChapter => self.seek_chapter(false),
            KeyAction::Fullscreen => self.toggle_fullscreen(),
            KeyAction::Leave => self.leave(),
            KeyAction::Skip => {
                if self.osd.skip.is_visible() {
                    self.skip();
                }
            }
        }
        self.show_osd();
        glib::Propagation::Stop
    }

    /// Reveals the OSD and cursor, then hides both after a quiet spell
    /// (unless a menu is open, or paused without "hide when paused").
    fn show_osd(self: &Rc<Self>) {
        self.osd.refresh_hints();
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
            let held_by_pause = inner.player.is_paused()
                && !crate::config::Settings::load()
                    .unwrap_or_default()
                    .playback
                    .hide_osd_when_paused;
            if held_by_pause || inner.osd.menu_open() || inner.controls_mode.get() {
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

    /// Stops the screen blanking and the system sleeping while a video
    /// plays (not music), if Preferences wants that.
    fn keep_screen_on(&self, playing: bool) {
        let wanted = playing
            && self.session().is_some_and(|s| !s.item.is_audio())
            && crate::config::Settings::load()
                .unwrap_or_default()
                .playback
                .keep_screen_on;
        crate::awake::set(wanted);
        let mut held = self.screen_inhibit.borrow_mut();
        if wanted == held.is_some() {
            return;
        }
        if let Some((app, cookie)) = held.take() {
            app.uninhibit(cookie);
        } else if let Some(window) = self.window()
            && let Some(app) = window.application()
        {
            let flags = gtk::ApplicationInhibitFlags::IDLE | gtk::ApplicationInhibitFlags::SUSPEND;
            let cookie = app.inhibit(Some(&window), flags, Some("Playing a video"));
            // 0: nothing on this system can inhibit (no portal or session manager).
            if cookie != 0 {
                *held = Some((app, cookie));
            }
        }
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
            // The standalone player is the only page: leaving it closes.
            && !nav.pop()
            && let Some(window) = self.window()
        {
            window.close();
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

/// Green Shaders button while any group has a preset on.
fn show_shaders_on(button: &gtk::MenuButton, on: bool) {
    if on {
        button.add_css_class("shaders-active");
    } else {
        button.remove_css_class("shaders-active");
    }
}

/// A menu entry: optional leading icon, label, then an optional mark and
/// trailing icon.
fn menu_row(
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
fn check_icon() -> gtk::Image {
    gtk::Image::from_icon_name(crate::ui::icons::WATCHED)
}

/// Focuses the entry of `page` whose check shows, else its first entry.
fn focus_checked(page: &gtk::Box) {
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

fn sleep_menu(episode: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    for choice in SLEEP_CHOICES {
        let item = gio::MenuItem::new(Some(&choice.label(episode)), None);
        item.set_action_and_target_value(Some("player.sleep"), Some(&choice.name().to_variant()));
        menu.append_item(&item);
    }
    menu
}

pub fn bar_fill_from_name(name: &str) -> BarFill {
    match name {
        "blur" => BarFill::Blur,
        "glow" => BarFill::Glow,
        _ => BarFill::Off,
    }
}

const SUB_SIZE_ROW: &str = "subtitle-size";
const SUB_TIMING_ROW: &str = "subtitle-timing";

/// "Label  −  value  +" for the subtitles menu; clicking the value resets
/// it. `step` changes the value by one step (±1) or resets it (0) and
/// returns the new text.
fn adjust_row(title: &str, current: String, step: impl Fn(i32) -> String + 'static) -> gtk::Box {
    let row = gtk::Box::builder()
        .spacing(6)
        .margin_start(12)
        .margin_end(6)
        .build();
    row.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .hexpand(true)
            .build(),
    );
    let value = gtk::Button::builder()
        .label(current)
        .tooltip_text("Reset")
        .css_classes(["flat", "numeric"])
        .width_request(72)
        .build();
    let step = std::rc::Rc::new(step);
    for (icon, tooltip, direction) in [
        ("list-remove-symbolic", "Less", -1),
        ("list-add-symbolic", "More", 1),
    ] {
        let button = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tooltip)
            .css_classes(["flat", "circular"])
            .build();
        let (step, value_button) = (step.clone(), value.clone());
        button.connect_clicked(move |_| value_button.set_label(&step(direction)));
        if direction < 0 {
            row.append(&button);
            row.append(&value);
        } else {
            row.append(&button);
        }
    }
    value.connect_clicked(move |button| button.set_label(&step(0)));
    row
}

fn subtitle_size_row(player: Player) -> gtk::Box {
    let text = |scale: f64| format!("{:.0}%", scale * 100.0);
    adjust_row("Size", text(player.subtitle_scale()), move |direction| {
        let scale = match direction {
            0 => 1.0,
            d => ((player.subtitle_scale() + 0.1 * f64::from(d)) * 10.0).round() / 10.0,
        };
        let scale = scale.clamp(player::SUB_SCALE_RANGE.0, player::SUB_SCALE_RANGE.1);
        if let Err(e) = player.set_subtitle_scale(scale) {
            tracing::warn!("{e:#}");
        }
        if let Err(e) = crate::config::Settings::update(|s| s.subtitles.scale = scale) {
            tracing::warn!("couldn't save the subtitle size: {e:#}");
        }
        text(scale)
    })
}

fn subtitle_timing_row(player: Player) -> gtk::Box {
    let text = |seconds: f64| {
        if seconds.abs() < 0.05 {
            "0.0 s".to_string()
        } else {
            format!("{seconds:+.1} s")
        }
    };
    adjust_row("Timing", text(player.subtitle_delay()), move |direction| {
        let seconds = match direction {
            0 => 0.0,
            d => ((player.subtitle_delay() + 0.1 * f64::from(d)) * 10.0).round() / 10.0,
        };
        if let Err(e) = player.set_subtitle_delay(seconds) {
            tracing::warn!("{e:#}");
        }
        text(seconds)
    })
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

/// Seconds of video the scrub target moves per second at `tilt`: squared,
/// for fine control near the centre; at full tilt it crosses the whole
/// video in 30 s, but never slower than a minute per second.
fn scrub_rate(tilt: f64, duration: f64) -> f64 {
    tilt * tilt.abs() * (duration / 30.0).max(60.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_speed_follows_tilt_and_length() {
        assert_eq!(scrub_rate(0.0, 7200.0), 0.0);
        // Full tilt: a 2 h film in 30 s, a short clip still a minute a second.
        assert_eq!(scrub_rate(1.0, 7200.0), 240.0);
        assert_eq!(scrub_rate(-1.0, 7200.0), -240.0);
        assert_eq!(scrub_rate(1.0, 600.0), 60.0);
        // A light touch is slow.
        assert_eq!(scrub_rate(0.25, 600.0), 3.75);
    }

    #[test]
    fn sleep_choices_round_trip_and_read_well() {
        for choice in SLEEP_CHOICES {
            assert_eq!(SleepChoice::from_name(&choice.name()), Some(choice));
        }
        assert_eq!(SleepChoice::from_name("bogus"), None);
        assert_eq!(SleepChoice::After(45).label(true), "45 minutes");
        assert_eq!(SleepChoice::After(60).label(true), "1 hour");
        assert_eq!(SleepChoice::EndOfVideo.label(true), "End of this episode");
        assert_eq!(SleepChoice::EndOfVideo.label(false), "End of this video");
    }

    #[test]
    fn sleep_remaining_rounds_up() {
        let secs = Duration::from_secs;
        assert_eq!(sleep_remaining(secs(0)), "1m");
        assert_eq!(sleep_remaining(secs(1)), "1m");
        assert_eq!(sleep_remaining(secs(60)), "1m");
        assert_eq!(sleep_remaining(secs(61)), "2m");
        assert_eq!(sleep_remaining(secs(23 * 60)), "23m");
        assert_eq!(sleep_remaining(secs(60 * 60)), "1h00m");
        assert_eq!(sleep_remaining(secs(90 * 60 - 1)), "1h30m");
    }
}
