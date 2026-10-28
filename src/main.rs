// Release builds on Windows are GUI programs: no console window behind them.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod account;
mod auth;
mod auto_downloads;
mod awake;
mod cli;
mod config;
mod controller;
mod db;
#[cfg(target_os = "linux")]
mod dnd;
mod downloads;
mod emby;
mod flavour;
mod frame_gen;
mod gamescope;
mod keys;
mod local;
mod logging;
mod music;
mod osk;
mod playback;
mod player;
mod remote;
mod runtime;
mod shaders;
mod smoke;
mod svp;
mod svp_profile;
mod tracking;
mod ui;
mod update;

use adw::prelude::*;
use gtk::{gio, glib};

pub(crate) const APP_ID: &str = "io.github.krakerz.EmbyClientPlus";
/// The name people see (window titles, Emby's device list, media controls).
pub const APP_NAME: &str = "Emby Client+";

fn main() -> glib::ExitCode {
    // A file/URL (or `--player <file|url>`) opens just the player, without
    // Emby; -v / -h answer and exit.
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            // mpv takes its paths as UTF-8 strings anyway.
            Err(arg) => {
                eprintln!("embyclientplus: argument {arg:?} isn't valid UTF-8 (see --help)");
                return glib::ExitCode::from(2);
            }
        }
    }
    let player_only = match cli::parse(args) {
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
        cli::Command::Unexpected(arg) => {
            eprintln!("embyclientplus: unexpected argument {arg} (see --help)");
            return glib::ExitCode::from(2);
        }
    };
    player::video_area::prefer_gl_renderer();
    gamescope::prepare_environment();
    svp::expose_vapoursynth();
    logging::init();
    svp::log_vapoursynth();
    update::clean_up_previous();
    let server = config::Settings::load().unwrap_or_default().server;
    emby::set_appear_as_browser(server.appear_as_browser());
    emby::set_server_version(&server.version);
    // Browser mode sends the server's own version, as Emby's web app does;
    // ask before the first request (briefly: the server may be offline).
    if server.appear_as_browser() && !server.url.is_empty() {
        let url = server.url.clone();
        runtime::block_on_timeout(
            async move { emby::auth::learn_server_version(&url).await },
            std::time::Duration::from_secs(2),
        );
    }

    // The app is unique per session (a second launch activates the running
    // one); the player-only mode is its own instance each time.
    let flags = if player_only.is_some() {
        gio::ApplicationFlags::NON_UNIQUE
    } else {
        gio::ApplicationFlags::default()
    };
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(flags)
        .build();
    app.connect_activate(move |app| {
        // A second launch lands here in the running process: show its window.
        if let Some(window) = app
            .active_window()
            .or_else(|| app.windows().into_iter().next())
        {
            window.present();
            return;
        }
        ui::icons::install();
        ui::app_menu::install(app, player_only.is_none());
        let result = match &player_only {
            Some(target) => player::Player::new(mpv_config_dir().as_deref()).map(|player| {
                apply_bar_fill(player);
                smoke::arm(app);
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
    let code = app.run_with_args::<&str>(&[]);
    // Release the application (and its bus name) first: a new copy started
    // while this one still owned the name would just activate it and exit.
    drop(app);
    update::spawn_pending_relaunch();
    smoke::exit_code().map_or(code, glib::ExitCode::from)
}

/// The user's mpv.conf, staged for the embedded player, when enabled in
/// Preferences (mpv reads its config only at start-up).
fn mpv_config_dir() -> Option<std::path::PathBuf> {
    let settings = config::Settings::load().unwrap_or_default();
    if !settings.playback.use_mpv_conf {
        return None;
    }
    let source = player::user_config::source_dir()?;
    let staging = config::cache_dir()?.join("mpv-config");
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
