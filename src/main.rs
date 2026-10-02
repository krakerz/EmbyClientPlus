mod auth;
mod config;
mod controller;
mod db;
mod emby;
mod frame_gen;
mod gamescope;
mod logging;
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
            None => player::Player::new(mpv_config_dir().as_deref())
                .map(|player| ui::window::build(app, player).present()),
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

fn build_standalone_player(app: &adw::Application, target: &str) -> anyhow::Result<()> {
    let player = player::Player::new(mpv_config_dir().as_deref())?;
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
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("EmbyClientPlus")
        .default_width(1280)
        .default_height(720)
        .content(&video)
        .build();

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
    player.load(target)
}
