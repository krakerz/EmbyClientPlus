//! Running under gamescope (SteamOS Game Mode, or a nested session).

use std::sync::OnceLock;

use crate::config::FrameGenBackend;
use crate::svp_profile::{Target, effective_target};

/// Whether this process runs inside gamescope. Gamescope sets
/// `GAMESCOPE_WAYLAND_DISPLAY` for its clients; the SteamOS session also
/// sets `XDG_CURRENT_DESKTOP=gamescope` and `SteamGamepadUI`.
pub fn detected() -> bool {
    detected_in(|name| std::env::var_os(name).map(|v| v.to_string_lossy().into_owned()))
}

fn detected_in(var: impl Fn(&str) -> Option<String>) -> bool {
    var("GAMESCOPE_WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || var("XDG_CURRENT_DESKTOP").is_some_and(|v| v.eq_ignore_ascii_case("gamescope"))
        || var("SteamGamepadUI").is_some()
}

/// What startup decided about GTK's no-vsync drawing.
struct NoVsync {
    applied: bool,
    wanted: bool,
    target: Option<Target>,
    /// The user's own `GDK_DEBUG` was left alone.
    own_debug: bool,
}

static NO_VSYNC: OnceLock<NoVsync> = OnceLock::new();

/// Whether GTK draws without vsync in this run: set by us at startup, or
/// by the user's own `GDK_DEBUG`.
pub fn no_vsync_applied() -> bool {
    NO_VSYNC.get().is_some_and(|n| n.applied)
}

/// Without vsync every running animation (the player controls fading,
/// revealers, focus transitions) redraws the window as fast as the GPU
/// allows, and that steals it from mpv and SVP: playback stutters while the
/// controls are open or navigated. Animations go off for such a run, so
/// the UI only redraws when something actually changes. Needs GTK started.
pub fn calm_animations() {
    if !no_vsync_applied() {
        return;
    }
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_enable_animations(false);
        tracing::info!("game mode: animations off (drawing without vsync)");
    }
}

/// SVP's target as it was when the app started: SVP picks up target changes
/// only after a restart, so this is the one it's producing in this run.
pub fn startup_target() -> Option<Target> {
    NO_VSYNC.get().and_then(|n| n.target)
}

/// Logs the startup decision; logging isn't up yet when it's made.
pub fn log_startup() {
    let Some(n) = NO_VSYNC.get() else { return };
    let target = n
        .target
        .map_or_else(|| "unknown".to_owned(), |t| t.to_string());
    tracing::info!(
        "game mode: no-vsync {} (SVP target {target}, wanted {}{})",
        if n.applied { "on" } else { "off" },
        n.wanted,
        if n.own_debug {
            ", GDK_DEBUG set by the user"
        } else {
            ""
        }
    );
}

/// Whether the app should draw without vsync for SVP's target: only a
/// target that can exceed 60 fps needs it. With no target known, assume it can.
pub fn needs_no_vsync(target: Option<Target>) -> bool {
    match target {
        Some(Target::ScreenHalf | Target::ScreenThird) => false,
        Some(Target::Fixed(fps)) => fps > 60,
        Some(_) | None => true,
    }
}

/// Picks a GTK setup that works in gamescope, unless the user chose one:
/// gamescope's Vulkan WSI layer aborts GTK's Vulkan renderer, and its
/// Xwayland is the reliable way in (Wayland is only exposed on request).
///
/// Must run before GTK initialises and before any other thread starts.
pub fn prepare_environment() {
    if !detected() {
        return;
    }
    for (name, value) in [("GSK_RENDERER", "ngl"), ("GDK_BACKEND", "x11")] {
        if std::env::var_os(name).is_none() {
            // SAFETY: called first thing in main, while single-threaded.
            unsafe { std::env::set_var(name, value) };
        }
    }
    // GTK's X11 backend gets no frame timings from gamescope and paces at
    // 60 Hz, which caps 120 fps SVP at 60. No vsync lifts that, at the cost
    // of drawing as often as it can, so it's on only when SVP's target can
    // go past 60 fps.
    let settings = crate::config::Settings::load().unwrap_or_default();
    let svp = settings.frame_gen.default_backend == FrameGenBackend::Svp;
    let target = effective_target(&settings);
    let wanted = svp && needs_no_vsync(target);
    let user_debug = std::env::var_os("GDK_DEBUG");
    let applied = match &user_debug {
        Some(value) => value.to_string_lossy().contains("no-vsync"),
        None => {
            if wanted {
                // SAFETY: as above.
                unsafe { std::env::set_var("GDK_DEBUG", "no-vsync") };
            }
            wanted
        }
    };
    let _ = NO_VSYNC.set(NoVsync {
        applied,
        wanted,
        target,
        own_debug: user_debug.is_some(),
    });
    // Gamescope's WSI layer hooks every Vulkan device in the process and
    // asserts on ones that never present (GTK's probe, mpv's Vulkan
    // decoding). We present through GL on Xwayland, so opt out of it.
    // SAFETY: as above.
    unsafe { std::env::set_var("ENABLE_GAMESCOPE_WSI", "0") };
}

/// Puts the device to sleep through logind, as the power menu does. Steam's
/// idle sleep doesn't follow a stopped video, so the sleep timer asks for it.
/// Polkit allows this without a password for the active local session.
#[cfg(target_os = "linux")]
pub fn suspend() {
    use gtk::gio;
    use gtk::glib::prelude::*;

    gio::bus_get(gio::BusType::System, gio::Cancellable::NONE, |bus| {
        let bus = match bus {
            Ok(bus) => bus,
            Err(e) => return tracing::warn!("no system bus to suspend through: {e}"),
        };
        bus.call(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            "Suspend",
            // Not interactive: there's nobody awake to answer a prompt.
            Some(&(false,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            10_000,
            gio::Cancellable::NONE,
            |result| match result {
                Ok(_) => tracing::info!("sleep timer: suspending"),
                Err(e) => tracing::warn!("sleep timer: suspend refused: {e}"),
            },
        );
    });
}

#[cfg(test)]
mod tests {
    use super::{Target, detected_in, needs_no_vsync};

    #[test]
    fn no_vsync_only_when_svp_can_pass_60() {
        assert!(!needs_no_vsync(Some(Target::ScreenHalf)));
        assert!(!needs_no_vsync(Some(Target::ScreenThird)));
        assert!(!needs_no_vsync(Some(Target::Fixed(60))));
        assert!(needs_no_vsync(Some(Target::Fixed(72))));
        assert!(needs_no_vsync(Some(Target::Screen)));
        assert!(needs_no_vsync(Some(Target::Movie3)));
        assert!(needs_no_vsync(None));
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn detects_nested_and_session_gamescope() {
        assert!(detected_in(env(&[(
            "GAMESCOPE_WAYLAND_DISPLAY",
            "gamescope-0"
        )])));
        assert!(detected_in(env(&[("XDG_CURRENT_DESKTOP", "gamescope")])));
        assert!(detected_in(env(&[("SteamGamepadUI", "1")])));
        assert!(!detected_in(env(&[("XDG_CURRENT_DESKTOP", "KDE")])));
    }
}
