//! Headless smoke test for packaged builds: with `EMBYCLIENTPLUS_SMOKE_TEST=1`
//! and `--player <url>`, report the GL driver and decoder once the first
//! video frame is on screen, then exit (0 = ok, 1 = playback failed,
//! 2 = no frame within [`TIMEOUT_SECS`]). Without the variable nothing here runs.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::player::Player;

const TIMEOUT_SECS: u32 = 20;
const UNSET: i32 = -1;

static ARMED: AtomicBool = AtomicBool::new(false);
static CODE: AtomicI32 = AtomicI32::new(UNSET);

/// Starts the watchdog when the test is requested. Call once the player exists.
pub fn arm(app: &adw::Application) {
    if std::env::var("EMBYCLIENTPLUS_SMOKE_TEST").as_deref() != Ok("1") {
        return;
    }
    // `quit` runs from mpv's event thread too, which has no handle to the app.
    app.set_default();
    ARMED.store(true, Ordering::Relaxed);
    glib::timeout_add_seconds_local_once(TIMEOUT_SECS, || {
        eprintln!("smoke: no video frame within {TIMEOUT_SECS} s");
        finish(2);
    });
}

/// Whether the render callback should still offer frames.
pub fn pending() -> bool {
    ARMED.load(Ordering::Relaxed) && CODE.load(Ordering::Relaxed) == UNSET
}

/// The exit code the test settled on, if it ran.
pub fn exit_code() -> Option<u8> {
    u8::try_from(CODE.load(Ordering::Relaxed)).ok()
}

/// Playback couldn't start or broke off (any thread).
pub fn failed(reason: &str) {
    if ARMED.load(Ordering::Relaxed) {
        eprintln!("smoke: playback failed: {reason}");
        finish(1);
    }
}

/// A frame was just rendered. Reports once mpv has a decoded picture;
/// `gl` gives (GL_RENDERER, GL_VERSION) of the current context.
pub fn frame(player: Player, gl: impl FnOnce() -> (String, String)) {
    let mpv = player.mpv();
    if mpv.get_property::<i64>("video-out-params/w").unwrap_or(0) <= 0 {
        return;
    }
    // Claim first: the render callback fires again before the app is gone.
    if CODE
        .compare_exchange(UNSET, 0, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    let (renderer, version) = gl();
    let hwdec = mpv
        .get_property::<String>("hwdec-current")
        .unwrap_or_else(|_| "no".into());
    let hwdec = if hwdec.is_empty() { "no".into() } else { hwdec };
    println!("smoke: GL_RENDERER={renderer}");
    println!("smoke: GL_VERSION={version}");
    println!("smoke: hwdec={hwdec}");
    println!(
        "smoke: vsscript={}",
        loaded_vsscript().unwrap_or_else(|| "unknown".into())
    );
    println!("smoke: flavour={}", crate::flavour::name());
    quit();
}

fn finish(code: i32) {
    if CODE
        .compare_exchange(UNSET, code, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        quit();
    }
}

/// Closes the windows (their handlers stop mpv and SVP) and ends the app.
fn quit() {
    glib::idle_add_once(|| {
        if let Some(app) = gio::Application::default().and_downcast::<gtk::Application>() {
            for window in app.windows() {
                window.close();
            }
            app.quit();
        }
    });
}

/// The VSScript library this process has loaded (mpv's vapoursynth filter
/// pulls it in), from the memory map; elsewhere what the launcher exported.
fn loaded_vsscript() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
        maps.lines()
            .filter_map(|line| line.split_once('/').map(|(_, path)| path))
            .find(|path| path.contains("libvsscript"))
            .map(|path| format!("/{path}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::env::var("EMBYCLIENTPLUS_VSSCRIPT").ok()
    }
}
