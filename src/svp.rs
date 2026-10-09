//! Whether SVP is installed and SVP Manager is running, so the player can
//! say why interpolation isn't happening instead of silently not doing it.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use crate::config::Settings;

/// Where SVP's installer puts it unless told otherwise.
#[cfg(target_os = "linux")]
pub fn default_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().join("SVP4"))
}

/// The 64-bit installer uses Program Files; older ones used Program
/// Files (x86).
#[cfg(windows)]
pub fn default_dir() -> Option<PathBuf> {
    let candidates = [
        ("ProgramFiles", r"C:\Program Files"),
        ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
    ]
    .map(|(var, fallback)| {
        PathBuf::from(std::env::var_os(var).unwrap_or_else(|| fallback.into())).join("SVP 4")
    });
    candidates
        .iter()
        .find(|dir| is_install(dir))
        .or(candidates.first())
        .cloned()
}

#[cfg(target_os = "macos")]
pub fn default_dir() -> Option<PathBuf> {
    Some(PathBuf::from("/Applications/SVP 4 Mac.app"))
}

/// How the default folder reads in Preferences.
pub fn default_dir_label() -> &'static str {
    if cfg!(windows) {
        r"C:\Program Files\SVP 4"
    } else if cfg!(target_os = "macos") {
        "/Applications/SVP 4 Mac.app"
    } else {
        "~/SVP4"
    }
}

/// The configured SVP folder (Preferences), else `$SVP_DIR`, else the
/// usual one.
pub fn install_dir() -> Option<PathBuf> {
    let configured = Settings::load().unwrap_or_default().frame_gen.svp_dir;
    if !configured.trim().is_empty() {
        return Some(PathBuf::from(configured.trim()));
    }
    std::env::var_os("SVP_DIR")
        .map(PathBuf::from)
        .or_else(default_dir)
}

/// SVP Manager's executable inside an install folder.
fn manager_path(dir: &std::path::Path) -> PathBuf {
    if cfg!(windows) {
        dir.join("SVPManager.exe")
    } else if cfg!(target_os = "macos") {
        dir.join("Contents/Resources/SVPManager")
    } else {
        dir.join("SVPManager")
    }
}

/// Whether `dir` looks like an SVP install.
pub fn is_install(dir: &std::path::Path) -> bool {
    manager_path(dir).exists()
}

pub fn installed() -> bool {
    install_dir().is_some_and(|dir| is_install(&dir))
}

/// Scans /proc for the SVPManager process (its `comm` is exactly that).
#[cfg(target_os = "linux")]
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

/// Looks through the running processes for SVP Manager.
#[cfg(not(target_os = "linux"))]
pub fn manager_running() -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    system.processes().values().any(|process| {
        let name = process.name().to_string_lossy();
        is_manager(name.strip_suffix(".exe").unwrap_or(&name))
    })
}

/// VapourSynth's scripting library as SVP ships it.
#[cfg(windows)]
const VSSCRIPT_NAMES: &[&str] = &["VSScript.dll"];
#[cfg(target_os = "macos")]
const VSSCRIPT_NAMES: &[&str] = &[
    "libvapoursynth-script.0.dylib",
    "libvapoursynth-script.dylib",
];

/// Tells the bundled VapourSynth stand-in (packaging/vsshim) where SVP's
/// own VSScript library is, so libmpv's vapoursynth filter uses it. Linux
/// builds link SVP's copy directly (build.rs, the launcher script).
///
/// Must run before libmpv initialises and before any other thread starts.
pub fn expose_vapoursynth() {
    #[cfg(not(target_os = "linux"))]
    {
        const VAR: &str = "EMBYCLIENTPLUS_VSSCRIPT";
        if std::env::var_os(VAR).is_some() {
            return;
        }
        let dirs = vsscript_search_dirs();
        match dirs
            .iter()
            .find_map(|dir| find_file(dir, VSSCRIPT_NAMES, 4))
        {
            Some(library) => {
                // SVP's VapourSynth (pip style) finds Python by searching
                // for python.exe on the program's folder, then PATH; SVP's
                // own mpv sits beside it, we don't. So its folder goes on PATH.
                #[cfg(windows)]
                if let Some(dir) = library.parent() {
                    let mut paths = vec![dir.to_path_buf()];
                    paths.extend(std::env::split_paths(
                        &std::env::var_os("PATH").unwrap_or_default(),
                    ));
                    if let Ok(path) = std::env::join_paths(paths) {
                        // SAFETY: called first thing in main, while single-threaded.
                        unsafe { std::env::set_var("PATH", path) };
                    }
                }
                // Logging isn't up yet; this is reported by `log_vapoursynth`.
                // SAFETY: called first thing in main, while single-threaded.
                unsafe { std::env::set_var(VAR, library) };
            }
            None => {
                let looked = dirs
                    .iter()
                    .map(|d| d.display().to_string())
                    .collect::<Vec<_>>();
                // SAFETY: as above.
                unsafe { std::env::set_var(NOT_FOUND_VAR, looked.join("; ")) };
            }
        }
    }
}

/// Set instead of `EMBYCLIENTPLUS_VSSCRIPT` when no VSScript library was
/// found: the folders searched, for the log.
#[cfg(not(target_os = "linux"))]
const NOT_FOUND_VAR: &str = "EMBYCLIENTPLUS_VSSCRIPT_SEARCHED";

/// Says in the log which VapourSynth SVP will get (once logging is up),
/// and points the VapourSynth stand-in's own log next to ours.
pub fn log_vapoursynth() {
    #[cfg(not(target_os = "linux"))]
    {
        if let Some(dir) = crate::logging::log_dir() {
            let path = dir.join("vapoursynth.log");
            let _ = std::fs::remove_file(&path);
            // SAFETY: still at startup, before libmpv or any other thread
            // reads the environment.
            unsafe { std::env::set_var("EMBYCLIENTPLUS_VSSHIM_LOG", path) };
        }
        if let Some(library) = std::env::var_os("EMBYCLIENTPLUS_VSSCRIPT") {
            tracing::info!("VapourSynth for SVP: {}", PathBuf::from(library).display());
        } else if let Some(searched) = std::env::var_os(NOT_FOUND_VAR) {
            tracing::warn!(
                "no VapourSynth for SVP (VSScript library not found in {}); SVP can't interpolate",
                searched.to_string_lossy()
            );
        }
    }
}

/// Where SVP's VapourSynth is: its own portable copy on Windows (under
/// `mpv64`); on macOS SVP uses Homebrew's (or the VapourSynth installer's).
#[cfg(windows)]
fn vsscript_search_dirs() -> Vec<PathBuf> {
    install_dir().into_iter().collect()
}

#[cfg(target_os = "macos")]
fn vsscript_search_dirs() -> Vec<PathBuf> {
    [
        "/opt/homebrew/opt/vapoursynth/lib",
        "/usr/local/opt/vapoursynth/lib",
        "/Library/Frameworks/VapourSynth.framework",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

/// The first file named one of `names` in `dir`, looking `depth` folders deep.
#[cfg(not(target_os = "linux"))]
fn find_file(dir: &std::path::Path, names: &[&str], depth: usize) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    let named = |path: &&PathBuf| {
        path.file_name()
            .is_some_and(|n| names.iter().any(|name| n.eq_ignore_ascii_case(name)))
    };
    if let Some(found) = entries.iter().find(|p| p.is_file() && named(p)) {
        return Some(found.clone());
    }
    if depth == 0 {
        return None;
    }
    entries
        .iter()
        .filter(|p| p.is_dir())
        .find_map(|p| find_file(p, names, depth - 1))
}

/// How long SVP Manager gets to exit on SIGTERM before it's killed.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// The SVP Manager this app started, if any; only that one gets stopped.
static STARTED: Mutex<Option<Child>> = Mutex::new(None);

/// Starts SVP Manager if it isn't running: at app launch (when auto-start
/// is on), so it's ready before any video, and again at playback if it
/// was closed. On the desktop it sits in the tray. In Game Mode it shares
/// gamescope's X display, but without Steam's game id it isn't part of
/// this app as far as gamescope is concerned, so it never takes focus.
pub fn ensure_running() {
    if manager_running() {
        return;
    }
    let Some(dir) = install_dir().filter(|dir| is_install(dir)) else {
        return;
    };
    let manager = manager_path(&dir);
    let in_gamescope = crate::gamescope::detected();
    // A headless gamescope around it (to hide it) cost GPU time while
    // playing, which showed as stutter on handhelds; it runs plainly now.
    let mut command = Command::new(&manager);
    command
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if in_gamescope {
        // Qt would otherwise try gamescope's Wayland socket.
        command.env("QT_QPA_PLATFORM", "xcb");
        // Without Steam's game id and overlay, gamescope doesn't treat SVP
        // Manager as part of this game: its windows stay behind ours and
        // never take focus.
        for name in [
            "SteamAppId",
            "SteamGameId",
            "SteamOverlayGameId",
            "STEAM_GAME_DISPLAY_0",
            "LD_PRELOAD",
            "ENABLE_GAMESCOPE_WSI",
        ] {
            command.env_remove(name);
        }
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
        // Politely first, so it can close cleanly; forcefully if it lingers.
        // (Windows has no polite signal for a GUI process: straight to kill.)
        #[cfg(unix)]
        // SAFETY: a plain signal to the child process we started.
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM)
        };
        let deadline = std::time::Instant::now() + STOP_GRACE;
        while std::time::Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
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
