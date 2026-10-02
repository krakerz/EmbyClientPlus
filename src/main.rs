// Emby, auth, overrides and settings layers aren't wired into the new GTK
// app until Phase 1 (see TODO.md).
#[allow(dead_code)]
mod auth;
#[allow(dead_code)]
mod config;
#[allow(dead_code)]
mod db;
#[allow(dead_code)]
mod emby;
#[allow(dead_code)]
mod frame_gen;
mod player;

use adw::prelude::*;
use gtk::{gdk, glib};
use player::Player;
use tracing_subscriber::EnvFilter;

const APP_ID: &str = "io.github.krakerz.EmbyClientPlus";

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let Some(target) = std::env::args().nth(1) else {
        eprintln!("usage: embyclientplus <file-or-url>");
        return glib::ExitCode::FAILURE;
    };

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        if let Err(e) = build_window(app, &target) {
            tracing::error!("{e:#}");
            app.quit();
        }
    });
    // argv is ours to handle; GTK would otherwise try to open the target itself.
    app.run_with_args::<&str>(&[])
}

fn build_window(app: &adw::Application, target: &str) -> anyhow::Result<()> {
    let player = Player::new()?;
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("EmbyClientPlus")
        .default_width(1280)
        .default_height(720)
        .content(&player::video_area::new(player))
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
