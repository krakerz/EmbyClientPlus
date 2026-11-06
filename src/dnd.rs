//! "Do not disturb" while a video is open (Linux): desktop notifications
//! are held back, then let through again afterwards. There's no standard
//! for it, so the first thing that works is used: your own commands
//! (Preferences), KDE Plasma's notification inhibition, the caelestia
//! shell, swaync, dunst, mako, or GNOME's banner setting. Whatever was
//! already in do-not-disturb mode is left that way afterwards.
//!
//! All switching happens on a single background worker so a backend probe
//! or a user command can never block the GTK main thread. `set()` just
//! sends a request; `release_blocking()` sends "off" and waits for it.

use std::io::read_to_string;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::config::Settings;

/// How long a backend probe or user command may run before we give up.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

/// How long `release_blocking` waits for the worker to acknowledge "off".
const RELEASE_TIMEOUT: Duration = Duration::from_secs(4);

/// How often to poll a running child.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

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

/// A request for the background worker. `Release` carries a channel the
/// caller waits on to learn when "off" has been applied.
enum Msg {
    Set(bool),
    Release(mpsc::Sender<()>),
}

/// The worker thread, started lazily on the first `set` or
/// `release_blocking` call. It applies requests one at a time, in order,
/// so an "on" followed quickly by an "off" can't run out of order.
static WORKER: OnceLock<mpsc::Sender<Msg>> = OnceLock::new();

fn worker() -> &'static mpsc::Sender<Msg> {
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || loop_worker(rx));
        tx
    })
}

fn loop_worker(rx: mpsc::Receiver<Msg>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Set(on) => apply(on),
            Msg::Release(ack) => {
                apply(false);
                let _ = ack.send(());
            }
        }
    }
}

/// Turns do-not-disturb on (`true`, if Preferences wants it and it isn't
/// already) or back off (`false`). Non-blocking: the work is handed to the
/// background worker and returns at once.
pub fn set(on: bool) {
    let _ = worker().send(Msg::Set(on));
}

/// Turns do-not-disturb off and waits for the worker to acknowledge it,
/// giving up after `RELEASE_TIMEOUT`. Used on app/window close so the
/// desktop isn't left in do-not-disturb mode.
pub fn release_blocking() {
    let (ack_tx, ack_rx) = mpsc::channel();
    let _ = worker().send(Msg::Release(ack_tx));
    let _ = ack_rx.recv_timeout(RELEASE_TIMEOUT);
}

/// Applies one request: enable, disable, or nothing if the state is
/// already what was asked for. Keeps the previous backend cache and held
/// state, just on the worker thread instead of the caller's.
fn apply(on: bool) {
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
    // Probing costs a D-Bus wait and several subprocesses, so what this
    // desktop has is looked up once per process (nothing included).
    static BACKEND: OnceLock<Option<Backend>> = OnceLock::new();
    if let Some(backend) = BACKEND.get() {
        return backend.and_then(|backend| attempt(backend).flatten());
    }
    let mut found = None;
    let mut held = None;
    for backend in Backend::ALL {
        if let Some(result) = attempt(backend) {
            found = Some(backend);
            held = result;
            break;
        }
    }
    let _ = BACKEND.set(found);
    held
}

#[derive(Clone, Copy)]
enum Backend {
    Plasma,
    Caelestia,
    Swaync,
    Dunst,
    Mako,
    Gnome,
}

impl Backend {
    /// In the order they're tried.
    const ALL: [Backend; 6] = [
        Backend::Plasma,
        Backend::Caelestia,
        Backend::Swaync,
        Backend::Dunst,
        Backend::Mako,
        Backend::Gnome,
    ];
}

/// `None`: the backend isn't on this desktop. Otherwise whether
/// do-not-disturb was switched on: it isn't when it was on already.
fn attempt(backend: Backend) -> Option<Option<Held>> {
    // Each: is it there, and is do-not-disturb off now? Then switch it on.
    Some(match backend {
        Backend::Plasma => return plasma_inhibit().map(Some),
        Backend::Caelestia => {
            let caelestia = ["qs", "-c", "caelestia", "ipc", "call", "notifs"];
            let enabled = output(&[&caelestia[..], &["isDndEnabled"]].concat())?;
            (enabled.trim() == "false" && run(&[&caelestia[..], &["enableDnd"]].concat()))
                .then_some(Held::Caelestia)
        }
        Backend::Swaync => {
            let enabled = output(&["swaync-client", "--get-dnd"])?;
            (enabled.trim() == "false" && run(&["swaync-client", "--dnd-on"]))
                .then_some(Held::Swaync)
        }
        Backend::Dunst => {
            let paused = output(&["dunstctl", "is-paused"])?;
            (paused.trim() == "false" && run(&["dunstctl", "set-paused", "true"]))
                .then_some(Held::Dunst)
        }
        Backend::Mako => {
            let modes = output(&["makoctl", "mode"])?;
            (!modes.lines().any(|m| m.trim() == "do-not-disturb")
                && run(&["makoctl", "mode", "-a", "do-not-disturb"]))
            .then_some(Held::Mako)
        }
        Backend::Gnome => {
            if !std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.contains("GNOME")) {
                return None;
            }
            let key = ["org.gnome.desktop.notifications", "show-banners"];
            let shown = output(&[&["gsettings", "get"][..], &key[..]].concat())?;
            (shown.trim() == "true"
                && run(&[&["gsettings", "set"][..], &key[..], &["false"]].concat()))
            .then_some(Held::Gnome)
        }
    })
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

/// Runs `argv`, giving up after `COMMAND_TIMEOUT` if it doesn't answer.
/// Returns true if the command exited successfully.
fn run(argv: &[&str]) -> bool {
    let mut child = match Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if start.elapsed() >= COMMAND_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    tracing::info!(
                        "do not disturb: {} didn't answer in {} s",
                        argv[0],
                        COMMAND_TIMEOUT.as_secs()
                    );
                    return false;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return false,
        }
    }
}

/// The command's output, if it exists and succeeds. Stdout is read on a
/// helper thread so a full pipe can't block the child; the whole probe
/// gives up after `COMMAND_TIMEOUT`.
fn output(argv: &[&str]) -> Option<String> {
    let mut child = match Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return None,
    };
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(read_to_string(&mut stdout).ok());
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = rx.recv_timeout(COMMAND_TIMEOUT).ok().flatten()?;
                return status.success().then_some(text);
            }
            Ok(None) => {
                if start.elapsed() >= COMMAND_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    tracing::info!(
                        "do not disturb: {} didn't answer in {} s",
                        argv[0],
                        COMMAND_TIMEOUT.as_secs()
                    );
                    return None;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_times_out_giving_none() {
        let start = Instant::now();
        let result = output(&["sh", "-c", "sleep 10"]);
        assert!(result.is_none());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn output_returns_stdout() {
        assert_eq!(output(&["sh", "-c", "echo hi"]), Some("hi\n".to_string()));
    }

    #[test]
    fn run_is_false_on_nonzero_exit() {
        assert!(!run(&["sh", "-c", "exit 1"]));
    }

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
