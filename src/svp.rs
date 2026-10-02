//! Whether SVP is installed and SVP Manager is running, so the player can
//! say why interpolation isn't happening instead of silently not doing it.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use crate::config::Settings;

/// Where SVP's installer puts it unless told otherwise.
pub fn default_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().join("SVP4"))
}

/// The configured SVP folder (Preferences), else `$SVP_DIR`, else `~/SVP4`.
pub fn install_dir() -> Option<PathBuf> {
    let configured = Settings::load().unwrap_or_default().frame_gen.svp_dir;
    if !configured.trim().is_empty() {
        return Some(PathBuf::from(configured.trim()));
    }
    std::env::var_os("SVP_DIR")
        .map(PathBuf::from)
        .or_else(default_dir)
}

/// Whether `dir` looks like an SVP install.
pub fn is_install(dir: &std::path::Path) -> bool {
    dir.join("SVPManager").exists()
}

pub fn installed() -> bool {
    install_dir().is_some_and(|dir| is_install(&dir))
}

/// Scans /proc for the SVPManager process (its `comm` is exactly that).
pub fn manager_running() -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let is_pid = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()));
        is_pid
            && std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| is_manager(&comm))
    })
}

/// The SVP Manager this app started, if any; only that one gets stopped.
static STARTED: Mutex<Option<Child>> = Mutex::new(None);

/// Starts SVP Manager if it isn't running. It normally sits in the tray
/// with no window; in gamescope (no tray) it runs invisibly on the
/// session's X display, which is verified to attach and interpolate.
pub fn ensure_running() {
    if manager_running() {
        return;
    }
    let Some(dir) = install_dir().filter(|dir| is_install(dir)) else {
        return;
    };
    let mut command = Command::new(dir.join("SVPManager"));
    command
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if crate::gamescope::detected() {
        // Qt would otherwise try gamescope's Wayland socket.
        command.env("QT_QPA_PLATFORM", "xcb");
    }
    // Our own gamescope workarounds are for this process only.
    command.env_remove("GDK_BACKEND").env_remove("GSK_RENDERER");
    match command.spawn() {
        Ok(child) => {
            tracing::info!("started SVP Manager (pid {})", child.id());
            if let Ok(mut started) = STARTED.lock() {
                *started = Some(child);
            }
        }
        Err(e) => tracing::warn!("couldn't start SVP Manager: {e}"),
    }
}

/// Stops the SVP Manager started by [`ensure_running`], at app exit.
pub fn stop_started() {
    let Ok(mut started) = STARTED.lock() else {
        return;
    };
    if let Some(mut child) = started.take() {
        let _ = child.kill();
        let _ = child.wait();
        tracing::info!("stopped the SVP Manager we started");
    }
}

fn is_manager(comm: &str) -> bool {
    comm.trim_end() == "SVPManager"
}

#[cfg(test)]
mod tests {
    use super::is_manager;

    #[test]
    fn matches_only_the_manager_process() {
        assert!(is_manager("SVPManager\n"));
        assert!(!is_manager("SVPManager2"));
        assert!(!is_manager("mpv"));
    }
}
