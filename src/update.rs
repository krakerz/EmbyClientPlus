//! Self-update from the latest published GitHub release.
//!
//! `releases/latest` only ever returns a published, non-prerelease release.
//! CI creates every release as a draft, so a build nobody has reviewed is
//! never offered here.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const LATEST_RELEASE: &str = "https://api.github.com/repos/krakerz/EmbyClientPlus/releases/latest";
const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// How this copy was installed, which decides what an update replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// The release archive: a folder holding `bin/` and `lib/` (Linux:
    /// installed by `install.sh`; Windows: unzipped anywhere, plus `share/`).
    Archive(PathBuf),
    /// A macOS app bundle (the `.app` folder).
    MacApp(PathBuf),
    /// An AppImage file.
    AppImage(PathBuf),
    /// Running from a build tree; updating doesn't apply.
    Source,
}

/// The archive's root on Windows and macOS holds this file, since no
/// launcher script sets `EMBYCLIENTPLUS_INSTALL` there.
const INSTALL_MARKER: &str = ".embyclientplus-install";

pub fn install_kind() -> InstallKind {
    let exe = std::env::current_exe().ok();
    if cfg!(target_os = "macos")
        && let Some(app) = exe.as_deref().and_then(|exe| {
            exe.ancestors()
                .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        })
        && app.join("Contents").join(INSTALL_MARKER).is_file()
    {
        return InstallKind::MacApp(app.to_path_buf());
    }
    if cfg!(windows)
        && let Some(root) = exe.as_deref().and_then(|exe| exe.parent()?.parent())
        && root.join(INSTALL_MARKER).is_file()
    {
        return InstallKind::Archive(root.to_path_buf());
    }
    if let Some(appimage) = std::env::var_os("APPIMAGE") {
        return InstallKind::AppImage(PathBuf::from(appimage));
    }
    // Set by the archive's launcher (packaging/embyclientplus.sh).
    if std::env::var_os("EMBYCLIENTPLUS_INSTALL").is_some_and(|kind| kind == "archive")
        && let Some(root) = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent()?.parent().map(Path::to_path_buf))
    {
        return InstallKind::Archive(root);
    }
    InstallKind::Source
}

pub fn current_version() -> &'static str {
    CURRENT
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub asset_name: String,
    asset_url: String,
    /// The asset's checksum (`sha256:<hex>`), checked after downloading.
    digest: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReleaseJson {
    tag_name: String,
    #[serde(default)]
    assets: Vec<AssetJson>,
}

#[derive(Debug, Clone, Deserialize)]
struct AssetJson {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>`, when GitHub reports one.
    #[serde(default)]
    digest: Option<String>,
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("EmbyClientPlus/{CURRENT}"))
        .build()
        .context("couldn't create the HTTP client")
}

/// The newer release for this install, if any.
pub async fn check(kind: &InstallKind) -> Result<Option<Update>> {
    let url =
        std::env::var("EMBYCLIENTPLUS_UPDATE_URL").unwrap_or_else(|_| LATEST_RELEASE.to_string());
    let release: ReleaseJson = client()?
        .get(&url)
        .send()
        .await
        .context("couldn't reach GitHub")?
        .error_for_status()
        .context("GitHub returned an error")?
        .json()
        .await
        .context("unexpected release data")?;
    Ok(pick_update(&release, kind, CURRENT))
}

fn pick_update(release: &ReleaseJson, kind: &InstallKind, current: &str) -> Option<Update> {
    let latest = parse_version(&release.tag_name)?;
    if latest <= parse_version(current)? {
        return None;
    }
    let suffix = match kind {
        InstallKind::Archive(_) => ARCHIVE_SUFFIX,
        InstallKind::MacApp(_) => "-macos-arm64.zip",
        InstallKind::AppImage(_) => "-x86_64.AppImage",
        InstallKind::Source => return None,
    };
    let asset = release.assets.iter().find(|a| a.name.ends_with(suffix))?;
    Some(Update {
        version: release.tag_name.trim_start_matches('v').to_string(),
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
        digest: asset.digest.clone(),
    })
}

/// The release archive for this OS.
const ARCHIVE_SUFFIX: &str = if cfg!(windows) {
    "-windows-x86_64.zip"
} else {
    "-linux-x86_64.tar.gz"
};

/// The program inside an unpacked archive's root.
const BUNDLE_PROGRAM: &str = if cfg!(windows) {
    "bin/embyclientplus.exe"
} else {
    "bin/embyclientplus-bin"
};

/// `1.2.3` / `v1.2.3` → (1, 2, 3); anything else is ignored.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    Some((major, minor, patch))
}

/// Downloads `update` and swaps it in. The running process keeps its
/// (now unlinked) files; a restart picks up the new version.
pub async fn apply(kind: &InstallKind, update: &Update, progress: &Progress) -> Result<()> {
    match kind {
        InstallKind::Archive(root) => apply_archive(root, update, progress).await,
        InstallKind::MacApp(app) => apply_mac_app(app, update, progress).await,
        InstallKind::AppImage(path) => apply_appimage(path, update, progress).await,
        InstallKind::Source => bail!("updates apply to installed builds only"),
    }
}

/// How far a download has got, shared with the UI (which polls it).
#[derive(Debug, Default)]
pub struct Progress {
    received: AtomicU64,
    /// 0 until known (the server may not say).
    total: AtomicU64,
}

impl Progress {
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> Option<u64> {
        Some(self.total.load(Ordering::Relaxed)).filter(|&t| t > 0)
    }

    /// 0..=1, when the size is known.
    pub fn fraction(&self) -> Option<f64> {
        self.total()
            .map(|total| (self.received() as f64 / total as f64).clamp(0.0, 1.0))
    }

    /// A new transfer of `total` bytes (0 when unknown) begins.
    pub fn start(&self, total: u64) {
        self.total.store(total, Ordering::Relaxed);
        self.received.store(0, Ordering::Relaxed);
    }

    pub fn add(&self, bytes: u64) {
        self.received.fetch_add(bytes, Ordering::Relaxed);
    }

    /// "34 % · 35.0 / 101.2 MB", or "35.0 MB" when the size is unknown.
    pub fn describe(&self) -> String {
        let mb = |bytes: u64| bytes as f64 / 1_000_000.0;
        match (self.fraction(), self.total()) {
            (Some(fraction), Some(total)) => format!(
                "{:.0} % · {:.1} / {:.1} MB",
                fraction * 100.0,
                mb(self.received()),
                mb(total)
            ),
            _ => format!("{:.1} MB", mb(self.received())),
        }
    }
}

/// Streams `url` to `to`, counting into `progress` as it goes, and checks
/// it against `digest` (`sha256:<hex>`) when there is one. A failed download
/// leaves no file behind.
async fn download(url: &str, to: &Path, progress: &Progress, digest: Option<&str>) -> Result<()> {
    let result = download_inner(url, to, progress, digest).await;
    if result.is_err() {
        let _ = std::fs::remove_file(to);
    }
    result
}

async fn download_inner(
    url: &str,
    to: &Path,
    progress: &Progress,
    digest: Option<&str>,
) -> Result<()> {
    use sha2::Digest;
    use std::io::Write;
    let mut response = client()?
        .get(url)
        .send()
        .await
        .context("download failed")?
        .error_for_status()
        .context("download failed")?;
    progress
        .total
        .store(response.content_length().unwrap_or(0), Ordering::Relaxed);
    progress.received.store(0, Ordering::Relaxed);
    let mut file = std::io::BufWriter::new(
        std::fs::File::create(to).with_context(|| format!("couldn't write {}", to.display()))?,
    );
    let mut hash = sha2::Sha256::new();
    while let Some(chunk) = response.chunk().await.context("download interrupted")? {
        file.write_all(&chunk)
            .with_context(|| format!("couldn't write {}", to.display()))?;
        hash.update(&chunk);
        progress
            .received
            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    file.flush()
        .with_context(|| format!("couldn't write {}", to.display()))?;
    if let Some(expected) = digest.and_then(|d| d.strip_prefix("sha256:")) {
        let actual: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            bail!("the download is corrupt (checksum mismatch)");
        }
    }
    Ok(())
}

/// Downloads `update` into `staging` (made fresh) and unpacks it there.
async fn fetch_and_unpack(staging: &Path, update: &Update, progress: &Progress) -> Result<()> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging)?;
    let archive = staging.join(&update.asset_name);
    download(
        &update.asset_url,
        &archive,
        progress,
        update.digest.as_deref(),
    )
    .await?;
    // bsdtar (Windows 10+, macOS) and GNU tar both pick the format themselves.
    let mut tar = Command::new("tar");
    tar.arg("-xf").arg(&archive).arg("-C").arg(staging);
    // No console flash from this console tool in the windowless release build.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        tar.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let status = tar.status().context("couldn't run tar")?;
    if !status.success() {
        bail!("couldn't unpack {}", update.asset_name);
    }
    let _ = std::fs::remove_file(&archive);
    Ok(())
}

async fn apply_archive(root: &Path, update: &Update, progress: &Progress) -> Result<()> {
    // Staged inside the install so the final renames never cross filesystems.
    let staging = root.join(".update");
    fetch_and_unpack(&staging, update, progress).await?;
    let unpacked = find_bundle(&staging)
        .with_context(|| format!("the update archive has no {BUNDLE_PROGRAM}"))?;
    if cfg!(windows) {
        install_windows(&unpacked, root).context("couldn't install the update")?;
    } else {
        swap_dirs(root, &unpacked, &staging, &["bin", "lib"])
            .context("couldn't install the update")?;
    }
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

/// Swaps `parts` (folders) of the install at `root` for those of `unpacked`,
/// the old ones going to `staging`. On an error everything already swapped is
/// put back rather than leave a half-updated install.
fn swap_dirs(root: &Path, unpacked: &Path, staging: &Path, parts: &[&str]) -> std::io::Result<()> {
    // (installed folder, where its old copy went), pushed before the new one
    // goes in so the one that failed is put back too.
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result = (|| {
        for part in parts {
            let old = staging.join(format!("old-{part}"));
            let current = root.join(part);
            let had = current.exists();
            if had {
                std::fs::rename(&current, &old)?;
            }
            done.push((current.clone(), had.then_some(old)));
            std::fs::rename(unpacked.join(part), &current)?;
        }
        Ok(())
    })();
    if result.is_err() {
        for (current, old) in done.into_iter().rev() {
            let _ = std::fs::remove_dir_all(&current);
            if let Some(old) = old {
                let _ = std::fs::rename(old, current);
            }
        }
    }
    result
}

const WINDOWS_LAUNCHER: &str = "Emby Client+.exe";

/// Suffix of files set aside by [`replace_files`]; removed on next start.
const SET_ASIDE: &str = "old-embyclientplus";

/// A file put in place: `dest` now holds the new file, and the one it
/// replaced (if any) waits at `aside`.
struct Swap {
    dest: PathBuf,
    aside: Option<PathBuf>,
}

/// Windows: swaps the unpacked release's `bin`, `lib`, `share` and launcher
/// into `root`, all or nothing.
fn install_windows(unpacked: &Path, root: &Path) -> std::io::Result<()> {
    let mut swaps = Vec::new();
    let result = (|| {
        // Windows won't move a folder with files in use (the running exe,
        // its DLLs), but it will rename those files: swap file by file.
        for part in ["bin", "lib", "share"] {
            if unpacked.join(part).exists() {
                replace_files(&unpacked.join(part), &root.join(part), &mut swaps)?;
            }
        }
        // The launcher beside bin\ (packaging/windows/launcher.c).
        let launcher = unpacked.join(WINDOWS_LAUNCHER);
        if launcher.is_file() {
            replace_file(&launcher, &root.join(WINDOWS_LAUNCHER), &mut swaps)?;
        }
        Ok(())
    })();
    if result.is_err() {
        undo_swaps(swaps);
    }
    result
}

/// Moves every file under `from` to the same place under `to`, renaming
/// any file already there out of the way first (it may be in use). Each
/// swap is recorded in `swaps`, to be undone with [`undo_swaps`].
fn replace_files(from: &Path, to: &Path, swaps: &mut Vec<Swap>) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    let mut entries = std::fs::read_dir(from)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            replace_files(&entry.path(), &dest, swaps)?;
        } else {
            replace_file(&entry.path(), &dest, swaps)?;
        }
    }
    Ok(())
}

fn replace_file(from: &Path, dest: &Path, swaps: &mut Vec<Swap>) -> std::io::Result<()> {
    let aside = if dest.exists() {
        let aside = set_aside_name(dest);
        let _ = std::fs::remove_file(&aside);
        std::fs::rename(dest, &aside)?;
        Some(aside)
    } else {
        None
    };
    // Recorded first: if the move fails, the original still has to come back.
    swaps.push(Swap {
        dest: dest.to_path_buf(),
        aside,
    });
    std::fs::rename(from, dest)
}

/// Puts every set-aside original back, newest swap first.
fn undo_swaps(swaps: Vec<Swap>) {
    for swap in swaps.into_iter().rev() {
        let _ = std::fs::remove_file(&swap.dest);
        if let Some(aside) = swap.aside {
            let _ = std::fs::rename(aside, swap.dest);
        }
    }
}

fn set_aside_name(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{SET_ASIDE}"));
    path.with_file_name(name)
}

/// Deletes files a previous update set aside (Windows; they were in use).
pub fn clean_up_previous() {
    let InstallKind::Archive(root) = install_kind() else {
        return;
    };
    if !cfg!(windows) {
        return;
    }
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(SET_ASIDE))
            {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// The folder in the unpacked archive holding bin/ and lib/ (the archive
/// wraps them in a versioned folder).
fn find_bundle(staging: &Path) -> Option<PathBuf> {
    let has_bundle = |dir: &Path| dir.join(BUNDLE_PROGRAM).is_file();
    if has_bundle(staging) {
        return Some(staging.to_path_buf());
    }
    std::fs::read_dir(staging)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|dir| has_bundle(dir))
}

/// Swaps the whole `.app` bundle: macOS lets a running app's bundle move.
async fn apply_mac_app(app: &Path, update: &Update, progress: &Progress) -> Result<()> {
    let parent = app
        .parent()
        .context("the app bundle has no parent folder")?;
    let staging = parent.join(".embyclientplus-update");
    fetch_and_unpack(&staging, update, progress).await?;
    let unpacked = std::fs::read_dir(&staging)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
        .context("the update archive has no .app")?;
    let old = staging.join("old.app");
    std::fs::rename(app, &old).context("couldn't move the old app aside")?;
    if let Err(e) = std::fs::rename(&unpacked, app) {
        let _ = std::fs::rename(&old, app);
        return Err(e).context("couldn't install the update");
    }
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

async fn apply_appimage(path: &Path, update: &Update, progress: &Progress) -> Result<()> {
    let partial = path.with_extension("AppImage.part");
    download(
        &update.asset_url,
        &partial,
        progress,
        update.digest.as_deref(),
    )
    .await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&partial, path).context("couldn't replace the AppImage")
}

/// The install to start once this process has exited (see
/// [`spawn_pending_relaunch`]).
static PENDING_RELAUNCH: std::sync::Mutex<Option<InstallKind>> = std::sync::Mutex::new(None);

/// Asks for the updated copy to start after this process exits: started now,
/// it would find this one still owning the app's bus name and just activate
/// it. The caller then quits.
pub fn relaunch(kind: &InstallKind) -> Result<()> {
    if *kind == InstallKind::Source {
        bail!("nothing to relaunch");
    }
    if let Ok(mut pending) = PENDING_RELAUNCH.lock() {
        *pending = Some(kind.clone());
    }
    Ok(())
}

/// Starts the copy [`relaunch`] asked for, if any; `main` calls it once the
/// application has shut down.
pub fn spawn_pending_relaunch() {
    let Some(kind) = PENDING_RELAUNCH.lock().ok().and_then(|mut p| p.take()) else {
        return;
    };
    if let Err(e) = spawn_updated(&kind) {
        tracing::warn!("{e:#}");
    }
}

fn spawn_updated(kind: &InstallKind) -> Result<()> {
    let program = match kind {
        InstallKind::Archive(root) if cfg!(windows) => root.join(BUNDLE_PROGRAM),
        InstallKind::Archive(root) => root.join("bin/embyclientplus"),
        InstallKind::MacApp(app) => {
            // A fresh instance of the bundle, not a reactivation of this one.
            Command::new("open")
                .arg("-n")
                .arg(app)
                .spawn()
                .context("couldn't start the updated app")?;
            return Ok(());
        }
        InstallKind::AppImage(path) => path.clone(),
        InstallKind::Source => bail!("nothing to relaunch"),
    };
    Command::new(&program)
        .spawn()
        .with_context(|| format!("couldn't start {}", program.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> ReleaseJson {
        ReleaseJson {
            tag_name: tag.into(),
            assets: vec![
                AssetJson {
                    name: format!("embyclientplus-{tag}-linux-x86_64.tar.gz"),
                    browser_download_url: "https://x/archive".into(),
                    digest: None,
                },
                AssetJson {
                    name: format!("embyclientplus-{tag}-windows-x86_64.zip"),
                    browser_download_url: "https://x/windows".into(),
                    digest: None,
                },
                AssetJson {
                    name: format!("EmbyClientPlus-{tag}-x86_64.AppImage"),
                    browser_download_url: "https://x/appimage".into(),
                    digest: None,
                },
            ],
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v0.10.2"), Some((0, 10, 2)));
        assert_eq!(parse_version("1.2.3-rc1"), Some((1, 2, 3)));
        assert_eq!(parse_version("nightly"), None);
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
    }

    #[test]
    fn picks_the_asset_for_the_install_kind() {
        let archive = InstallKind::Archive("/x".into());
        let appimage = InstallKind::AppImage("/x.AppImage".into());
        let update = pick_update(&release("0.6.0"), &archive, "0.5.0").unwrap();
        assert_eq!(update.version, "0.6.0");
        assert!(update.asset_name.ends_with(ARCHIVE_SUFFIX));
        let update = pick_update(&release("v0.6.0"), &appimage, "0.5.0").unwrap();
        assert!(update.asset_name.ends_with(".AppImage"));
        assert_eq!(update.version, "0.6.0");
    }

    #[test]
    fn same_or_older_releases_and_source_builds_get_nothing() {
        let archive = InstallKind::Archive("/x".into());
        assert!(pick_update(&release("0.5.0"), &archive, "0.5.0").is_none());
        assert!(pick_update(&release("0.4.9"), &archive, "0.5.0").is_none());
        assert!(pick_update(&release("0.6.0"), &InstallKind::Source, "0.5.0").is_none());
    }

    #[test]
    fn finds_the_bundle_inside_a_versioned_folder() {
        let root =
            std::env::temp_dir().join(format!("embyclientplus-update-{}", std::process::id()));
        let bundle = root.join("EmbyClientPlus-0.6.0-x86_64");
        std::fs::create_dir_all(bundle.join("bin")).unwrap();
        std::fs::write(bundle.join(BUNDLE_PROGRAM), b"").unwrap();
        assert_eq!(find_bundle(&root), Some(bundle));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn replacing_files_sets_the_old_ones_aside() {
        let root = std::env::temp_dir().join(format!("embyclientplus-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (new, current) = (root.join("new"), root.join("current"));
        std::fs::create_dir_all(new.join("sub")).unwrap();
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(new.join("app.exe"), b"new").unwrap();
        std::fs::write(new.join("sub/data"), b"new").unwrap();
        std::fs::write(current.join("app.exe"), b"old").unwrap();
        replace_files(&new, &current, &mut Vec::new()).unwrap();
        assert_eq!(std::fs::read(current.join("app.exe")).unwrap(), b"new");
        assert_eq!(std::fs::read(current.join("sub/data")).unwrap(), b"new");
        assert_eq!(
            std::fs::read(set_aside_name(&current.join("app.exe"))).unwrap(),
            b"old"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_file_swap_restores_every_replaced_file() {
        let root = std::env::temp_dir().join(format!("embyclientplus-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (new, current) = (root.join("new"), root.join("current"));
        std::fs::create_dir_all(new.join("sub")).unwrap();
        std::fs::create_dir_all(&current).unwrap();
        std::fs::write(new.join("a.dll"), b"new").unwrap();
        std::fs::write(new.join("b.dll"), b"new").unwrap();
        std::fs::write(new.join("sub/data"), b"new").unwrap();
        std::fs::write(current.join("a.dll"), b"old").unwrap();
        std::fs::write(current.join("b.dll"), b"old").unwrap();
        // A file where a folder has to go: the swap fails after a and b.
        std::fs::write(current.join("sub"), b"in the way").unwrap();
        let mut swaps = Vec::new();
        assert!(replace_files(&new, &current, &mut swaps).is_err());
        undo_swaps(swaps);
        assert_eq!(std::fs::read(current.join("a.dll")).unwrap(), b"old");
        assert_eq!(std::fs::read(current.join("b.dll")).unwrap(), b"old");
        assert!(!set_aside_name(&current.join("a.dll")).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_folder_swap_restores_every_folder() {
        let root = std::env::temp_dir().join(format!("embyclientplus-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (install, unpacked, staging) = (
            root.join("install"),
            root.join("unpacked"),
            root.join("stage"),
        );
        for dir in ["bin", "lib"] {
            std::fs::create_dir_all(install.join(dir)).unwrap();
            std::fs::write(install.join(dir).join("f"), b"old").unwrap();
        }
        // The new archive has bin but no lib, so the second swap fails.
        std::fs::create_dir_all(unpacked.join("bin")).unwrap();
        std::fs::write(unpacked.join("bin/f"), b"new").unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        assert!(swap_dirs(&install, &unpacked, &staging, &["bin", "lib"]).is_err());
        assert_eq!(std::fs::read(install.join("bin/f")).unwrap(), b"old");
        assert_eq!(std::fs::read(install.join("lib/f")).unwrap(), b"old");
        // And a good one swaps both (the failed try moved `bin` out of here).
        std::fs::create_dir_all(unpacked.join("bin")).unwrap();
        std::fs::create_dir_all(unpacked.join("lib")).unwrap();
        std::fs::write(unpacked.join("lib/f"), b"new").unwrap();
        std::fs::write(unpacked.join("bin/f"), b"new").unwrap();
        swap_dirs(&install, &unpacked, &staging, &["bin", "lib"]).unwrap();
        assert_eq!(std::fs::read(install.join("bin/f")).unwrap(), b"new");
        assert_eq!(std::fs::read(install.join("lib/f")).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn release_assets_carry_an_optional_digest() {
        let json = r#"{"tag_name":"v9.9.9","assets":[
            {"name":"x-linux-x86_64.tar.gz","browser_download_url":"https://x/a","digest":"sha256:ab12"},
            {"name":"x-x86_64.AppImage","browser_download_url":"https://x/b"}]}"#;
        let release: ReleaseJson = serde_json::from_str(json).unwrap();
        assert_eq!(release.assets[0].digest.as_deref(), Some("sha256:ab12"));
        assert_eq!(release.assets[1].digest, None);
    }

    /// End-to-end check against a fake release server (not run by default):
    /// `EMBYCLIENTPLUS_UPDATE_URL=http://127.0.0.1:8765/latest.json
    /// EMBYCLIENTPLUS_UPDATE_TEST_ROOT=<installed root> cargo test -- --ignored`
    #[test]
    #[ignore]
    fn update_from_fake_release() {
        let root = PathBuf::from(std::env::var("EMBYCLIENTPLUS_UPDATE_TEST_ROOT").unwrap());
        let kind = InstallKind::Archive(root.clone());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let found = runtime
            .block_on(check(&kind))
            .unwrap()
            .expect("the fake release should be newer");
        runtime
            .block_on(apply(&kind, &found, &Progress::default()))
            .unwrap();
        assert!(root.join("bin/embyclientplus-bin").is_file());
        assert!(!root.join(".update").exists());
    }

    #[test]
    fn progress_describes_known_and_unknown_sizes() {
        let progress = Progress::default();
        progress.received.store(35_000_000, Ordering::Relaxed);
        assert_eq!(progress.fraction(), None);
        assert_eq!(progress.describe(), "35.0 MB");
        progress.total.store(100_000_000, Ordering::Relaxed);
        assert_eq!(progress.fraction(), Some(0.35));
        assert_eq!(progress.describe(), "35 % · 35.0 / 100.0 MB");
    }

    /// Streams a real download and checks the count (not run by default):
    /// `EMBYCLIENTPLUS_TEST_DOWNLOAD_URL=http://127.0.0.1:8766/big.bin
    /// cargo test download_counts -- --ignored`
    #[test]
    #[ignore]
    fn download_counts_progress() {
        let url = std::env::var("EMBYCLIENTPLUS_TEST_DOWNLOAD_URL").unwrap();
        let to = std::env::temp_dir().join(format!("embyclientplus-dl-{}", std::process::id()));
        let progress = Progress::default();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(download(&url, &to, &progress, None))
            .unwrap();
        let size = std::fs::metadata(&to).unwrap().len();
        assert_eq!(progress.received(), size);
        assert_eq!(progress.total(), Some(size));
        assert_eq!(progress.fraction(), Some(1.0));
        let _ = std::fs::remove_file(&to);
    }
}
