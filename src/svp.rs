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
        // The bundle itself, or (since Finder's folder picker can't choose
        // inside an app) the folder holding it, e.g. /Applications.
        let bundle = if dir.extension().is_some_and(|ext| ext == "app") {
            dir.to_path_buf()
        } else {
            dir.join("SVP 4 Mac.app")
        };
        bundle.join("Contents/MacOS/SVPManager")
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

/// What the VapourSynth stand-in (packaging/vsshim) reads: the VSScript
/// library to load, and one to load instead if that one fails.
const VSSCRIPT_VAR: &str = "EMBYCLIENTPLUS_VSSCRIPT";
#[cfg(not(target_os = "macos"))]
const FALLBACK_VAR: &str = "EMBYCLIENTPLUS_VSSCRIPT_FALLBACK";

/// SVP's VSScript library on Linux.
#[cfg(target_os = "linux")]
fn svp_vsscript() -> Option<PathBuf> {
    install_dir()
        .map(|dir| dir.join("mpv/libvapoursynth-script.so.0"))
        .filter(|path| path.is_file())
}

/// Where an installed (non-SVP) VapourSynth's VSScript library may be:
/// distro packages (pip style, or the classic soname), then
/// `pip install --user`.
#[cfg(target_os = "linux")]
fn system_vsscript_candidates() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = [
        "/usr/lib/libvsscript.so",
        "/usr/lib64/libvsscript.so",
        "/usr/local/lib/libvsscript.so",
        "/usr/lib/x86_64-linux-gnu/libvsscript.so",
        "/usr/lib/libvapoursynth-script.so.0",
        "/usr/lib64/libvapoursynth-script.so.0",
        "/usr/local/lib/libvapoursynth-script.so.0",
        "/usr/lib/x86_64-linux-gnu/libvapoursynth-script.so.0",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect();
    if let Some(lib) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".local/lib"))
        && let Ok(entries) = std::fs::read_dir(lib)
    {
        candidates.extend(entries.filter_map(Result::ok).map(|entry| {
            entry
                .path()
                .join("site-packages/vapoursynth/libvsscript.so")
        }));
    }
    candidates
}

/// Where a separately installed VapourSynth's VSScript.dll may be: the
/// VapourSynth installer's folder, then pip installs into python.org's
/// Pythons (per user, all users, `pip install --user`).
#[cfg(windows)]
fn system_vsscript_candidates() -> Vec<PathBuf> {
    let env_dir = |var: &str| std::env::var_os(var).map(PathBuf::from);
    let pythons = |root: Option<PathBuf>, inner: &str| -> Vec<PathBuf> {
        root.and_then(|root| std::fs::read_dir(root).ok())
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .starts_with("python")
            })
            .map(|entry| entry.path().join(inner))
            .collect()
    };
    let mut candidates = Vec::new();
    if let Some(dir) = env_dir("ProgramFiles") {
        candidates.push(dir.join(r"VapourSynth\core\VSScript.dll"));
        candidates.push(dir.join(r"VapourSynth\VSScript.dll"));
    }
    let site = r"Lib\site-packages\vapoursynth\VSScript.dll";
    candidates.extend(pythons(
        env_dir("LOCALAPPDATA").map(|d| d.join(r"Programs\Python")),
        site,
    ));
    candidates.extend(pythons(env_dir("ProgramFiles"), site));
    candidates.extend(pythons(
        env_dir("APPDATA").map(|d| d.join("Python")),
        r"site-packages\vapoursynth\VSScript.dll",
    ));
    candidates
}

/// The installed (non-SVP) VapourSynth's VSScript library. On Linux it is
/// resolved to its real path, because pip-style VapourSynth looks itself
/// up in vapoursynth.toml by the path it was loaded from and repairs a
/// stale entry only for that one.
#[cfg(not(target_os = "macos"))]
pub fn system_vapoursynth() -> Option<PathBuf> {
    let svp = install_dir();
    #[cfg(target_os = "linux")]
    let svp = svp.and_then(|dir| dir.canonicalize().ok());
    system_vsscript_candidates()
        .into_iter()
        .filter_map(|path| {
            #[cfg(target_os = "linux")]
            let path = path.canonicalize().ok()?;
            path.is_file().then_some(path)
        })
        .find(|path| svp.as_ref().is_none_or(|svp| !path.starts_with(svp)))
}

/// Why the System VapourSynth choice can't be used here, if it can't.
#[cfg(not(target_os = "macos"))]
pub fn system_vapoursynth_unavailable() -> Option<&'static str> {
    #[cfg(target_os = "linux")]
    if !shim_present() {
        return Some("This build loads SVP's VapourSynth directly");
    }
    if system_vapoursynth().is_none() {
        return Some(if cfg!(windows) {
            "No VapourSynth found besides SVP's"
        } else {
            "No system VapourSynth found (install your distribution's vapoursynth package)"
        });
    }
    None
}

/// The library the System preference picks at startup, when it's set and usable.
#[cfg(not(target_os = "macos"))]
fn system_vapoursynth_choice() -> Option<PathBuf> {
    let source = Settings::load().unwrap_or_default().frame_gen.vapoursynth;
    if source != crate::config::VapourSynthSource::System {
        return None;
    }
    #[cfg(target_os = "linux")]
    if !shim_present() {
        return None;
    }
    system_vapoursynth()
}

#[cfg(target_os = "linux")]
type LoadedFn = unsafe extern "C" fn() -> *const libc::c_char;

/// The stand-in's report function, when libmpv got the stand-in. The
/// compat AppImage hands libmpv SVP's library directly.
#[cfg(target_os = "linux")]
fn shim_loaded_fn() -> Option<LoadedFn> {
    // SAFETY: a NUL-terminated name, looked up in the global scope.
    let symbol =
        unsafe { libc::dlsym(libc::RTLD_DEFAULT, c"embyclientplus_vsshim_loaded".as_ptr()) };
    // SAFETY: packaging/vsshim defines it as `const char *(void)`.
    (!symbol.is_null())
        .then(|| unsafe { std::mem::transmute::<*mut libc::c_void, LoadedFn>(symbol) })
}

#[cfg(target_os = "linux")]
fn shim_present() -> bool {
    shim_loaded_fn().is_some()
}

/// The VSScript library the stand-in loaded, once SVP's filter has run in
/// this session, for Preferences.
#[cfg(target_os = "linux")]
pub fn active_vapoursynth() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let loaded = shim_loaded_fn()?;
    // SAFETY: NULL, or a NUL-terminated string the stand-in never changes
    // after publishing it.
    let path = unsafe { loaded() };
    if path.is_null() {
        return None;
    }
    // SAFETY: checked non-NULL above.
    let bytes = unsafe { std::ffi::CStr::from_ptr(path) }.to_bytes();
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(windows)]
pub fn active_vapoursynth() -> Option<PathBuf> {
    None
}

/// Linux: SVP's VSScript library, or with the System preference the
/// installed one, with SVP's as the stand-in's fallback.
#[cfg(target_os = "linux")]
fn expose_vapoursynth_linux() {
    if std::env::var_os(VSSCRIPT_VAR).is_some() {
        return;
    }
    let svp = svp_vsscript();
    // SAFETY (all): called first thing in main, while single-threaded.
    match system_vapoursynth_choice() {
        Some(system) => unsafe {
            std::env::set_var(VSSCRIPT_VAR, system);
            if let Some(svp) = svp {
                std::env::set_var(FALLBACK_VAR, svp);
            }
        },
        None => {
            if let Some(svp) = svp {
                unsafe { std::env::set_var(VSSCRIPT_VAR, svp) };
            }
        }
    }
}

/// VapourSynth's scripting library as SVP ships it.
#[cfg(windows)]
const VSSCRIPT_NAMES: &[&str] = &["VSScript.dll"];
#[cfg(target_os = "macos")]
const VSSCRIPT_NAMES: &[&str] = &[
    "libvapoursynth-script.0.dylib",
    "libvapoursynth-script.dylib",
    // pip-style VapourSynth (what `vapoursynth get-vsscript` reports).
    "libvsscript.dylib",
];

/// Tells the bundled VapourSynth stand-in (packaging/vsshim) which VSScript
/// library libmpv's vapoursynth filter uses: SVP's own, or with the System
/// preference (Linux, Windows) the installed one, with SVP's as the fallback.
///
/// Must run before libmpv initialises and before any other thread starts.
pub fn expose_vapoursynth() {
    #[cfg(target_os = "linux")]
    expose_vapoursynth_linux();
    #[cfg(not(target_os = "linux"))]
    {
        const VAR: &str = VSSCRIPT_VAR;
        // mpv builds with VapourSynth 74+ support (Homebrew's) load VSScript
        // at run time from VSSCRIPT_PATH, not through the stand-in; one the
        // user set wins.
        const MPV_VAR: &str = "VSSCRIPT_PATH";
        if std::env::var_os(VAR).is_some() {
            return;
        }
        #[cfg(windows)]
        let system = system_vapoursynth_choice();
        let user_set = std::env::var_os(MPV_VAR)
            .map(PathBuf::from)
            .filter(|path| path.is_file());
        let dirs = vsscript_search_dirs();
        match user_set.or_else(|| {
            dirs.iter()
                .find_map(|dir| find_file(dir, VSSCRIPT_NAMES, 4))
        }) {
            Some(library) => {
                // SVP's VapourSynth (pip style) finds Python by searching
                // for python.exe on the program's folder, then PATH; SVP's
                // own mpv sits beside it, we don't. So its folder goes on PATH,
                // unless the installed VapourSynth comes first: then that one
                // must find its own Python, and the stand-in adds SVP's folder
                // only if it falls back.
                #[cfg(windows)]
                if system.is_none()
                    && let Some(dir) = library.parent()
                {
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
                unsafe {
                    std::env::set_var(MPV_VAR, &library);
                    std::env::set_var(VAR, &library);
                }
                #[cfg(windows)]
                if let Some(system) = system {
                    // SAFETY: as above.
                    unsafe {
                        std::env::set_var(VAR, system);
                        std::env::set_var(FALLBACK_VAR, &library);
                    }
                }
            }
            None => {
                #[cfg(windows)]
                if let Some(system) = system {
                    // SAFETY: as above.
                    unsafe { std::env::set_var(VAR, system) };
                    return;
                }
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

/// When no VapourSynth was found for SVP at startup, what to do about it
/// (SVP attaches but can't interpolate without one). On Linux SVP not
/// being installed is reported elsewhere.
pub fn vapoursynth_hint() -> Option<&'static str> {
    #[cfg(target_os = "linux")]
    if MANAGER_EXITED_AT_ONCE.load(std::sync::atomic::Ordering::Relaxed) {
        return Some(COMPAT_GLIBC_HINT);
    }
    #[cfg(not(target_os = "linux"))]
    if std::env::var_os("EMBYCLIENTPLUS_VSSCRIPT").is_none() {
        return Some(if cfg!(target_os = "macos") {
            "SVP needs VapourSynth: install it with “brew install vapoursynth”, then restart the app"
        } else {
            "SVP's VapourSynth wasn't found: reinstall SVP 4, then restart the app"
        });
    }
    None
}

/// Says in the log which VapourSynth SVP will get (once logging is up),
/// and points the VapourSynth stand-in's own log next to ours.
pub fn log_vapoursynth() {
    #[cfg(target_os = "linux")]
    if !shim_present() {
        tracing::info!("VapourSynth for SVP: SVP's, linked directly (no stand-in in this build)");
        return;
    }
    if let Some(dir) = crate::logging::log_dir() {
        let path = dir.join("vapoursynth.log");
        let _ = std::fs::remove_file(&path);
        // SAFETY: still at startup, before libmpv or any other thread
        // reads the environment.
        unsafe { std::env::set_var("EMBYCLIENTPLUS_VSSHIM_LOG", path) };
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(fallback) = std::env::var_os(FALLBACK_VAR) {
        tracing::info!(
            "VapourSynth for SVP: the system's, {} (SVP's {} if it fails; see vapoursynth.log)",
            PathBuf::from(std::env::var_os(VSSCRIPT_VAR).unwrap_or_default()).display(),
            PathBuf::from(fallback).display()
        );
        return;
    }
    if let Some(library) = std::env::var_os(VSSCRIPT_VAR) {
        tracing::info!("VapourSynth for SVP: {}", PathBuf::from(library).display());
    } else {
        #[cfg(not(target_os = "linux"))]
        if let Some(searched) = std::env::var_os(NOT_FOUND_VAR) {
            tracing::warn!(
                "no VapourSynth for SVP (VSScript library not found in {}); SVP can't interpolate",
                searched.to_string_lossy()
            );
        }
        #[cfg(target_os = "linux")]
        tracing::warn!(
            "no VapourSynth for SVP (SVP's VSScript library not found); SVP can't interpolate"
        );
    }
}

/// Where SVP's VapourSynth is: its own portable copy on Windows (under
/// `mpv64`); on macOS SVP uses Homebrew's (or the VapourSynth installer's).
#[cfg(windows)]
fn vsscript_search_dirs() -> Vec<PathBuf> {
    install_dir().into_iter().collect()
}

/// SVP for Mac uses Homebrew's VapourSynth (or MacPorts'), and asks it
/// where its scripting library is with `vapoursynth get-vsscript`; so do
/// we, then fall back to the usual library folders. Apps started from
/// Finder don't get Homebrew's PATH, hence the full paths.
#[cfg(target_os = "macos")]
fn vsscript_search_dirs() -> Vec<PathBuf> {
    let reported = [
        "/opt/homebrew/bin/vapoursynth",
        "/usr/local/bin/vapoursynth",
        "/opt/local/bin/vapoursynth",
    ]
    .into_iter()
    .find_map(|tool| {
        let output = Command::new(tool).arg("get-vsscript").output().ok()?;
        let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        (output.status.success() && path.is_file()).then(|| path.parent().map(PathBuf::from))?
    });
    reported
        .into_iter()
        .chain(
            [
                "/opt/homebrew/opt/vapoursynth/lib",
                // R74+ lives in a Python venv: libexec/lib/python3.x/site-packages/vapoursynth.
                "/opt/homebrew/opt/vapoursynth/libexec",
                "/opt/homebrew/lib",
                "/usr/local/opt/vapoursynth/lib",
                "/usr/local/opt/vapoursynth/libexec",
                "/usr/local/lib",
                "/opt/local/lib",
                "/Library/Frameworks/VapourSynth.framework",
            ]
            .into_iter()
            .map(PathBuf::from),
        )
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
#[cfg(unix)]
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// The SVP Manager this app started, if any; only that one gets stopped.
static STARTED: Mutex<Option<Child>> = Mutex::new(None);

/// The compat AppImage bundles its own glibc, but SVP's libraries come from
/// the host and need 2.38+; on older systems the Manager dies right away.
#[cfg(target_os = "linux")]
const COMPAT_GLIBC_HINT: &str =
    "SVP needs a newer system (glibc 2.38 or later) in the compatibility AppImage";

/// Set when, in the compat flavour, the Manager we started exited within
/// [`EARLY_EXIT_WINDOW`]; `vapoursynth_hint` then surfaces the toast.
#[cfg(target_os = "linux")]
static MANAGER_EXITED_AT_ONCE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(target_os = "linux")]
const EARLY_EXIT_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);

/// Compat only: watches the Manager we just started for a quick exit.
#[cfg(target_os = "linux")]
fn watch_for_early_exit() {
    use std::sync::atomic::Ordering;
    MANAGER_EXITED_AT_ONCE.store(false, Ordering::Relaxed);
    std::thread::spawn(|| {
        let deadline = std::time::Instant::now() + EARLY_EXIT_WINDOW;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let Ok(mut started) = STARTED.lock() else {
                return;
            };
            // Taken by `stop_started`: we're quitting.
            let Some(child) = started.as_mut() else {
                return;
            };
            if let Ok(Some(status)) = child.try_wait() {
                // A launcher that hands over to a surviving process is fine.
                if !manager_running() {
                    tracing::warn!("SVP Manager exited at once ({status}); {COMPAT_GLIBC_HINT}");
                    MANAGER_EXITED_AT_ONCE.store(true, Ordering::Relaxed);
                }
                return;
            }
        }
    });
}

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
            #[cfg(target_os = "linux")]
            if crate::flavour::is_compat() {
                watch_for_early_exit();
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
        {
            // SAFETY: a plain signal to the child process we started.
            unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
            let deadline = std::time::Instant::now() + STOP_GRACE;
            while std::time::Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
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
