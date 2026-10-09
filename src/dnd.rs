//! "Do not disturb" while a video is open (Linux): desktop notifications
//! are held back, then let through again afterwards. There's no standard
//! for it, so the first thing that works is used: your own commands
//! (Preferences), KDE Plasma's notification inhibition, the caelestia
//! shell, swaync, dunst, mako, or GNOME's banner setting. Whatever was
//! already in do-not-disturb mode is left that way afterwards.

use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::config::Settings;

/// How do-not-disturb was turned on, so it can be turned off the same way.
enum Held {
    /// The user's "on" command ran; this is their "off" command.
    Custom(String),
    /// KDE: inhibited over this D-Bus connection (closing it also ends it).
    Plasma(zbus::Connection, u32),
    Caelestia,
    Swaync,
    Dunst,
    Mako,
    Gnome,
}

static HELD: Mutex<Option<Held>> = Mutex::new(None);

/// Turns do-not-disturb on (`true`, if Preferences wants it and it isn't
/// already) or back off (`false`).
pub fn set(on: bool) {
    let Ok(mut held) = HELD.lock() else { return };
    if on && held.is_none() {
        let settings = Settings::load().unwrap_or_default().playback;
        if settings.do_not_disturb {
            *held = enable(&settings.dnd_on_command, &settings.dnd_off_command);
            if held.is_none() {
                tracing::info!("do not disturb: nothing on this desktop could be switched");
            }
        }
    } else if !on && let Some(state) = held.take() {
        disable(state);
    }
}

fn enable(on_command: &str, off_command: &str) -> Option<Held> {
    if !on_command.trim().is_empty() {
        return shell(on_command).then(|| Held::Custom(off_command.to_string()));
    }
    if let Some(held) = plasma_inhibit() {
        return Some(held);
    }
    // Each: is it there, and is do-not-disturb off now? Then switch it on.
    let caelestia = ["qs", "-c", "caelestia", "ipc", "call", "notifs"];
    if let Some(enabled) = output(&[&caelestia[..], &["isDndEnabled"]].concat()) {
        return (enabled.trim() == "false" && run(&[&caelestia[..], &["enableDnd"]].concat()))
            .then_some(Held::Caelestia);
    }
    if let Some(enabled) = output(&["swaync-client", "--get-dnd"]) {
        return (enabled.trim() == "false" && run(&["swaync-client", "--dnd-on"]))
            .then_some(Held::Swaync);
    }
    if let Some(paused) = output(&["dunstctl", "is-paused"]) {
        return (paused.trim() == "false" && run(&["dunstctl", "set-paused", "true"]))
            .then_some(Held::Dunst);
    }
    if let Some(modes) = output(&["makoctl", "mode"]) {
        return (!modes.lines().any(|m| m.trim() == "do-not-disturb")
            && run(&["makoctl", "mode", "-a", "do-not-disturb"]))
        .then_some(Held::Mako);
    }
    let gnome = std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.contains("GNOME"));
    if gnome {
        let key = ["org.gnome.desktop.notifications", "show-banners"];
        let shown = output(&[&["gsettings", "get"][..], &key[..]].concat())?;
        return (shown.trim() == "true"
            && run(&[&["gsettings", "set"][..], &key[..], &["false"]].concat()))
        .then_some(Held::Gnome);
    }
    None
}

#[cfg(test)]
fn describe(state: &Held) -> &'static str {
    match state {
        Held::Custom(_) => "custom",
        Held::Plasma(..) => "plasma",
        Held::Caelestia => "caelestia",
        Held::Swaync => "swaync",
        Held::Dunst => "dunst",
        Held::Mako => "mako",
        Held::Gnome => "gnome",
    }
}

fn disable(state: Held) {
    let ok = match state {
        Held::Custom(command) => command.trim().is_empty() || shell(&command),
        Held::Plasma(connection, cookie) => crate::runtime::block_on_timeout(
            async move {
                connection
                    .call_method(
                        Some("org.freedesktop.Notifications"),
                        "/org/freedesktop/Notifications",
                        Some("org.freedesktop.Notifications"),
                        "UnInhibit",
                        &cookie,
                    )
                    .await
                    .is_ok()
            },
            std::time::Duration::from_secs(2),
        )
        .unwrap_or(false),
        Held::Caelestia => run(&[
            "qs",
            "-c",
            "caelestia",
            "ipc",
            "call",
            "notifs",
            "disableDnd",
        ]),
        Held::Swaync => run(&["swaync-client", "--dnd-off"]),
        Held::Dunst => run(&["dunstctl", "set-paused", "false"]),
        Held::Mako => run(&["makoctl", "mode", "-r", "do-not-disturb"]),
        Held::Gnome => run(&[
            "gsettings",
            "set",
            "org.gnome.desktop.notifications",
            "show-banners",
            "true",
        ]),
    };
    if !ok {
        tracing::warn!("do not disturb: couldn't turn it back off");
    }
}

/// KDE Plasma (and anything else implementing the spec's `Inhibit`):
/// notifications stay held while the returned connection is open.
fn plasma_inhibit() -> Option<Held> {
    crate::runtime::block_on_timeout(
        async {
            let connection = zbus::Connection::session().await.ok()?;
            let hints: std::collections::HashMap<&str, zbus::zvariant::Value> =
                std::collections::HashMap::new();
            let reply = connection
                .call_method(
                    Some("org.freedesktop.Notifications"),
                    "/org/freedesktop/Notifications",
                    Some("org.freedesktop.Notifications"),
                    "Inhibit",
                    &(crate::APP_ID, "Watching a video", hints),
                )
                .await
                .ok()?;
            let cookie: u32 = reply.body().deserialize().ok()?;
            Some(Held::Plasma(connection, cookie))
        },
        std::time::Duration::from_secs(2),
    )
    .flatten()
}

/// Runs a user's command line through the shell; true if it succeeded.
fn shell(command: &str) -> bool {
    run(&["sh", "-c", command])
}

fn run(argv: &[&str]) -> bool {
    Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The command's output, if it exists and succeeds.
fn output(argv: &[&str]) -> Option<String> {
    let output = Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    /// Switches this desktop's do-not-disturb on and back off (not run by
    /// default: it really does): `cargo test dnd -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn switches_this_desktop_on_and_off() {
        let held = super::enable("", "").expect("no do-not-disturb found here");
        println!("switched on: {}", super::describe(&held));
        std::thread::sleep(std::time::Duration::from_secs(2));
        super::disable(held);
    }
}
