mod auth;
mod cli;
mod config;
mod controller;
mod db;
mod downloads;
mod emby;
mod frame_gen;
mod gamescope;
mod keys;
mod local;
mod logging;
mod music;
mod playback;
mod player;
mod remote;
mod runtime;
mod shaders;
mod svp;
mod ui;
mod update;

use adw::prelude::*;
use gtk::glib;

const APP_ID: &str = "io.github.krakerz.EmbyClientPlus";
/// The name people see (window titles, Emby's device list, media controls).
pub const APP_NAME: &str = "Emby Client+";

fn main() -> glib::ExitCode {
    // A file/URL (or `--player <file|url>`) opens just the player, without
    // Emby; -v / -h answer and exit.
    let player_only = match cli::parse(std::env::args().skip(1)) {
        cli::Command::Run => None,
        cli::Command::Player { target } => Some(target),
        cli::Command::Version => {
            println!("{}", cli::version());
            return glib::ExitCode::SUCCESS;
        }
        cli::Command::Help => {
            println!("{}", cli::help());
            return glib::ExitCode::SUCCESS;
        }
        cli::Command::Unknown(option) => {
            eprintln!("embyclientplus: unknown option {option} (see --help)");
            return glib::ExitCode::from(2);
        }
    };
    gamescope::prepare_environment();
    logging::init();

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        ui::icons::install();
        let result = match &player_only {
            Some(target) => player::Player::new(mpv_config_dir().as_deref()).map(|player| {
                apply_bar_fill(player);
                ui::present_standalone(app, player, target.as_deref());
            }),
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
