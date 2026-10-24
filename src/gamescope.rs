//! Running under gamescope (SteamOS Game Mode, or a nested session).

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

/// Picks a GTK setup that works in gamescope, unless the user chose one:
/// gamescope's Vulkan WSI layer aborts GTK's Vulkan renderer, and its
/// Xwayland is the reliable way in (Wayland is only exposed on request).
/// gamescope sends GTK's X11 backend no frame timings, so GTK paces its
/// redraws at an assumed 60 Hz and caps video there (SVP's 120 fps showed
/// as ~60 on a 120 Hz handheld); `no-vsync` lets mpv's frames through and
/// gamescope still limits presentation to the display.
///
/// Must run before GTK initialises and before any other thread starts.
pub fn prepare_environment() {
    if !detected() {
        return;
    }
    for (name, value) in [
        ("GSK_RENDERER", "ngl"),
        ("GDK_BACKEND", "x11"),
        ("GDK_DEBUG", "no-vsync"),
    ] {
        if std::env::var_os(name).is_none() {
            // SAFETY: called first thing in main, while single-threaded.
            unsafe { std::env::set_var(name, value) };
        }
    }
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
    use super::detected_in;

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
