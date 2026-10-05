//! `--player`: just the player, mpv-style, for local files and links. A
//! start page takes dropped files and links, opens files or URLs, and lists
//! what's in the downloads folder; playing uses the same player page as for
//! Emby titles (SVP, shaders, picture options, tracks), with the next video
//! in the folder after each one. Leaving the player goes back to the start.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use super::player_page::{Handlers, PlayerPage};
use crate::playback::{LocalMedia, PlaybackSession, Quality};
use crate::player::Player;

struct State {
    page: PlayerPage,
    player: Player,
    nav: adw::NavigationView,
    session: RefCell<Option<Rc<PlaybackSession>>>,
    toasts: adw::ToastOverlay,
    /// The start page's list of videos in the downloads folder.
    library: gtk::Box,
}

pub fn present(app: &adw::Application, player: Player, target: Option<&str>) {
    let settings = crate::config::Settings::load().unwrap_or_default();
    super::window::load_css();
    super::window::apply_theme(settings.window.theme);
    super::apply_ui_scale(settings.window.ui_scale);

    let page = PlayerPage::new(player);
    let nav = adw::NavigationView::new();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&nav));
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(crate::APP_NAME)
        .default_width(settings.window.width.max(640))
        .default_height(settings.window.height.max(400))
        .content(&toasts)
        .build();
    if super::window::fullscreen_locked() {
        window.connect_map(|window| window.fullscreen());
    }
    let library = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    let state = Rc::new(State {
        page,
        player,
        nav: nav.clone(),
        session: RefCell::new(None),
        toasts,
        library,
    });
    nav.add(&start_page(&state));

    // Files and links dropped anywhere in the window play (replacing what's on).
    let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    drop.set_types(&[gdk::FileList::static_type(), glib::Type::STRING]);
    drop.connect_drop({
        let state = Rc::downgrade(&state);
        move |_, value, _, _| {
            let Some(state) = state.upgrade() else {
                return false;
            };
            let target = if let Ok(files) = value.get::<gdk::FileList>() {
                files
                    .files()
                    .first()
                    .and_then(|f| f.path())
                    .map(|p| p.to_string_lossy().into_owned())
            } else {
                value
                    .get::<String>()
                    .ok()
                    .and_then(|text| dropped_text(&text))
            };
            match target {
                Some(target) => {
                    play(&state, crate::local::media(&target));
                    true
                }
                None => false,
            }
        }
    });
    window.add_controller(drop);

    crate::controller::start({
        let (state, window) = (Rc::downgrade(&state), window.downgrade());
        move |pad, repeat| {
            if let (Some(state), Some(window)) = (state.upgrade(), window.upgrade()) {
                window.set_focus_visible(true);
                let playing = state.nav.visible_page().as_ref() == Some(state.page.page());
                if playing
                    && window.visible_dialog().is_none()
                    && super::gamepad::player_press(&window, &state.page, pad, repeat)
                {
                    return;
                }
                // Menu / Start on the start page: Preferences.
                if !playing
                    && window.visible_dialog().is_none()
                    && crate::controller::action_for(pad, crate::controller::Context::Browse)
                        == Some(crate::controller::Action::Preferences)
                {
                    super::preferences::show_player_only(&window);
                    return;
                }
                super::gamepad::handle(&window, None, pad, repeat);
            }
        }
    });
    window.connect_close_request({
        let state = state.clone();
        move |_| {
            if let Some(session) = state.session.take() {
                session.stop();
            }
            if let Err(e) = state.player.quit() {
                tracing::warn!("{e:#}");
            }
            crate::svp::stop_started();
            glib::Propagation::Proceed
        }
    });
    window.present();
    if let Some(target) = target {
        play(&state, crate::local::media(target));
    }
}

/// A dropped text: a link, a `file://` URI, or a plain path (first line).
fn dropped_text(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    if line.starts_with("file://") {
        return gio::File::for_uri(line)
            .path()
            .map(|p| p.to_string_lossy().into_owned());
    }
    Some(line.to_string())
}

/// The idle screen: logo, "drop here", Open File / Open URL, downloads.
fn start_page(state: &Rc<State>) -> adw::NavigationPage {
    let header = adw::HeaderBar::new();
    let preferences = gtk::Button::builder()
        .icon_name(crate::ui::icons::SETTINGS)
        .tooltip_text("Preferences")
        .build();
    preferences.connect_clicked(super::preferences::show_player_only);
    header.pack_end(&preferences);

    let logo = gtk::Image::builder()
        .icon_name(crate::ui::icons::APP)
        .pixel_size(128)
        .build();
    let hint = gtk::Label::builder()
        .label("Drop files or URLs to play here")
        .css_classes(["title-2"])
        .build();
    let buttons = gtk::Box::builder()
        .spacing(12)
        .halign(gtk::Align::Center)
        .build();
    // File choosers don't show up in gamescope.
    if !crate::gamescope::detected() {
        let open = gtk::Button::builder()
            .label("Open File…")
            .css_classes(["pill", "suggested-action"])
            .build();
        open.connect_clicked({
            let state = Rc::downgrade(state);
            move |button| {
                if let Some(state) = state.upgrade() {
                    open_file(&state, button);
                }
            }
        });
        buttons.append(&open);
    }
    let url = gtk::Button::builder()
        .label("Open URL…")
        .css_classes(["pill"])
        .build();
    url.connect_clicked({
        let state = Rc::downgrade(state);
        move |button| {
            if let Some(state) = state.upgrade() {
                open_url(&state, button);
            }
        }
    });
    buttons.append(&url);

    let welcome = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .margin_top(36)
        .margin_bottom(24)
        .build();
    welcome.append(&logo);
    welcome.append(&hint);
    welcome.append(&buttons);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(24)
        .build();
    content.append(&welcome);
    content.append(&state.library);
    let clamp = adw::Clamp::builder()
        .maximum_size(900)
        .child(&content)
        .build();
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .vexpand(true)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scrolled));
    let page = adw::NavigationPage::builder()
        .child(&toolbar)
        .title(crate::APP_NAME)
        .tag("start")
        .build();
    // The list is fresh each time the start page shows (after a download
    // folder change in Preferences, or a file added meanwhile).
    page.connect_showing({
        let state = Rc::downgrade(state);
        move |_| {
            if let Some(state) = state.upgrade() {
                fill_library(&state);
            }
        }
    });
    fill_library(state);
    page
}

/// "From your downloads": every video in the downloads folder, named from
/// its Emby details when it has them.
fn fill_library(state: &Rc<State>) {
    super::clear(&state.library);
    let folder = crate::downloads::folder();
    let mut files = Vec::new();
    collect_videos(&folder, &mut files, 0);
    if files.is_empty() {
        return;
    }
    let titled: Vec<(String, PathBuf)> = {
        let downloads = crate::downloads::list();
        let mut titled: Vec<(String, PathBuf)> = files
            .into_iter()
            .map(|path| {
                let title = downloads
                    .iter()
                    .find(|d| d.file == path)
                    .map(|d| match &d.item.series_name {
                        Some(series) if d.item.item_type == "Episode" => {
                            format!("{series} · {}", d.item.episode_label())
                        }
                        _ => d.item.name.clone(),
                    })
                    .unwrap_or_else(|| {
                        path.file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    });
                (title, path)
            })
            .collect();
        titled.sort_by_key(|(title, _)| title.to_lowercase());
        titled
    };
    state.library.append(
        &gtk::Label::builder()
            .label("From your downloads")
            .xalign(0.0)
            .css_classes(["heading"])
            .build(),
    );
    state.library.append(
        &gtk::Label::builder()
            .label(folder.to_string_lossy())
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    for (title, path) in titled {
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&title))
            .subtitle(super::format_size(size))
            .activatable(true)
            .build();
        row.add_suffix(&gtk::Image::from_icon_name(crate::ui::icons::PLAY));
        let state = Rc::downgrade(state);
        row.connect_activated(move |_| {
            if let Some(state) = state.upgrade() {
                play(&state, media_for(&path));
            }
        });
        list.append(&row);
    }
    state.library.append(&list);
}

fn collect_videos(dir: &Path, found: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(|e| e.ok().map(|e| e.path())) {
        if path.is_dir() {
            collect_videos(&path, found, depth + 1);
        } else if crate::local::is_video(&path) {
            found.push(path);
        }
    }
}

/// A file to play, titled from its Emby details when it was downloaded.
fn media_for(path: &Path) -> LocalMedia {
    let mut media = crate::local::media(&path.to_string_lossy());
    if let Some(download) = crate::downloads::list()
        .into_iter()
        .find(|d| d.file == path)
    {
        // Its name, episode numbers and chapters, not its id: the id keeps
        // pointing at the file, for previous/next in the folder.
        let item = download.item;
        media.item.name = item.name;
        media.item.item_type = item.item_type;
        media.item.series_name = item.series_name;
        media.item.parent_index_number = item.parent_index_number;
        media.item.index_number = item.index_number;
        media.item.chapters = item.chapters;
    }
    media
}

fn open_file(state: &Rc<State>, anchor: &gtk::Button) {
    let dialog = gtk::FileDialog::builder()
        .title("Open a video")
        .modal(true)
        .build();
    let window = anchor.root().and_downcast::<gtk::Window>();
    let state = Rc::downgrade(state);
    dialog.open(window.as_ref(), None::<&gio::Cancellable>, move |result| {
        let (Some(state), Some(path)) = (state.upgrade(), result.ok().and_then(|f| f.path()))
        else {
            return; // cancelled
        };
        play(&state, media_for(&path));
    });
}

fn open_url(state: &Rc<State>, anchor: &gtk::Button) {
    let entry = gtk::Entry::builder()
        .placeholder_text("https://…")
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading("Open URL")
        .body("A video file or stream link, or a video page (with yt-dlp installed)")
        .extra_child(&entry)
        .default_response("open")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("open", "Open")]);
    dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);
    let state = Rc::downgrade(state);
    dialog.connect_response(None, move |_, response| {
        let link = entry.text().trim().to_string();
        if response == "open"
            && !link.is_empty()
            && let Some(state) = state.upgrade()
        {
            play(&state, crate::local::media(&link));
        }
    });
    dialog.present(Some(anchor));
}

fn play(state: &Rc<State>, media: LocalMedia) {
    if let Some(session) = state.session.take() {
        session.stop();
    }
    state
        .page
        .prepare(&media.item, Quality::Original, handlers(state));
    if state.nav.visible_page().as_ref() != Some(state.page.page()) {
        state.nav.push(state.page.page());
    }
    let state = state.clone();
    glib::spawn_future_local(async move {
        match PlaybackSession::start_local(state.player, media, 0).await {
            Ok(session) => {
                state.session.replace(Some(session.clone()));
                state.page.attach(session, None);
            }
            Err(e) => {
                tracing::error!("playback failed: {e:#}");
                toast(&state, &format!("Couldn't play this: {e}"));
                state.nav.pop();
            }
        }
    });
}

fn handlers(state: &Rc<State>) -> Handlers {
    let (play_next, stopped, toaster) = (
        Rc::downgrade(state),
        Rc::downgrade(state),
        Rc::downgrade(state),
    );
    Handlers {
        // Previous/next in the folder, and autoplay after each video.
        play: Box::new(move |item| {
            if let (Some(state), Some(path)) = (play_next.upgrade(), crate::local::path_of(item)) {
                play(&state, media_for(Path::new(path)));
            }
        }),
        change_quality: Box::new(|_| {}),
        // Back on the start page: stop.
        stopped: Box::new(move || {
            if let Some(session) = stopped.upgrade().and_then(|s| s.session.take()) {
                session.stop();
            }
        }),
        toast: Box::new(move |message| {
            if let Some(state) = toaster.upgrade() {
                toast(&state, message);
            }
        }),
    }
}

fn toast(state: &State, message: &str) {
    state.toasts.add_toast(adw::Toast::new(message));
}
