mod auth;
mod config;
mod controller;
mod db;
mod emby;
mod frame_gen;
mod gamescope;
mod logging;
mod music;
mod playback;
mod player;
mod remote;
mod runtime;
mod svp;
mod ui;
mod update;

use adw::prelude::*;
use gtk::{gdk, glib};

const APP_ID: &str = "io.github.krakerz.EmbyClientPlus";
/// The name people see (window titles, Emby's device list, media controls).
pub const APP_NAME: &str = "Emby Client+";

fn main() -> glib::ExitCode {
    gamescope::prepare_environment();
    logging::init();

    // An argument plays that file/URL directly, skipping Emby (handy for
    // checking the player and SVP on their own).
    let target = std::env::args().nth(1);

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        ui::icons::install();
        let result = match &target {
            Some(target) => build_standalone_player(app, target),
            None => player::Player::new(mpv_config_dir().as_deref()).map(|player| {
                apply_bar_fill(player);
                ui::window::present(&ui::window::build(app, player))
            }),
        };
        if let Err(e) = result {
            tracing::error!("{e:#}");
            app.quit();
        }
    });
    // argv is ours to handle; GTK would otherwise try to open the target itself.
    app.run_with_args::<&str>(&[])
}

/// The user's mpv.conf, staged for the embedded player, when enabled in
/// Preferences (mpv reads its config only at start-up).
fn mpv_config_dir() -> Option<std::path::PathBuf> {
    let settings = config::Settings::load().unwrap_or_default();
    if !settings.playback.use_mpv_conf {
        return None;
    }
    let source = player::user_config::source_dir()?;
    let staging = directories::ProjectDirs::from("com", "krakerz", "embyclientplus")?
        .cache_dir()
        .join("mpv-config");
    let keep_hwdec = settings.frame_gen.default_backend != config::FrameGenBackend::Svp;
    match player::user_config::stage(&source, &staging, keep_hwdec) {
        Ok(dir) => Some(dir),
        Err(e) => {
            tracing::warn!("couldn't use {}: {e}", source.join("mpv.conf").display());
            None
        }
    }
}

/// The saved black-bar fill, applied once the player exists.
fn apply_bar_fill(player: player::Player) {
    let name = config::Settings::load()
        .unwrap_or_default()
        .playback
        .bar_fill;
    if let Err(e) = player.set_bar_fill(ui::player_page::bar_fill_from_name(&name)) {
        tracing::warn!("{e:#}");
    }
}

fn build_standalone_player(app: &adw::Application, target: &str) -> anyhow::Result<()> {
    let player = player::Player::new(mpv_config_dir().as_deref())?;
    apply_bar_fill(player);
    // Standalone mode always offers itself to SVP.
    let settings = config::Settings::load().unwrap_or_default();
    player.set_svp(settings.frame_gen.socket(), true)?;
    let video = player::video_area::new(player);
    let weak = glib::SendWeakRef::from(video.downgrade());
    player.on_event(move |event| {
        if event == player::PlayerEvent::Geometry {
            let weak = weak.clone();
            glib::MainContext::default().invoke(move || {
                if let Some(video) = weak.upgrade() {
                    player::video_area::refit(player, &video);
                }
            });
        }
    });
    let window_settings = config::Settings::load().unwrap_or_default().window;
    ui::window::apply_theme(window_settings.theme);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(APP_NAME)
        .default_width(window_settings.width.max(640))
        .default_height(window_settings.height.max(400))
        .content(&video)
        .build();
    if window_settings.start_fullscreen(gamescope::detected()) {
        window.connect_map(|window| window.fullscreen());
    }

    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, key, _, _| {
        let result = match key {
            gdk::Key::space => player.toggle_pause(),
            gdk::Key::Left => player.seek_relative(-5),
            gdk::Key::Right => player.seek_relative(5),
            _ => return glib::Propagation::Proceed,
        };
        if let Err(e) = result {
            tracing::warn!("{e:#}");
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);

    window.connect_close_request(move |_| {
        if let Err(e) = player.quit() {
            tracing::warn!("{e:#}");
        }
        glib::Propagation::Proceed
    });

    window.present();
    // Video-site pages go through yt-dlp, as trailers do in the app.
    let stream = remote::resolve(target, &remote::Options::from_settings(&settings))?;
    let subtitles: Vec<&str> = stream.subtitle.as_deref().into_iter().collect();
    player.load_at(&stream.url, 0.0, &subtitles, stream.audio.as_deref())
}
