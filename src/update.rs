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
    /// `install.sh` from the archive: a folder holding `bin/` and `lib/`.
    Archive(PathBuf),
    /// An AppImage file.
    AppImage(PathBuf),
    /// Running from a build tree; updating doesn't apply.
    Source,
}

pub fn install_kind() -> InstallKind {
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
        InstallKind::Archive(_) => "-linux-x86_64.tar.gz",
        InstallKind::AppImage(_) => "-x86_64.AppImage",
        InstallKind::Source => return None,
    };
    let asset = release.assets.iter().find(|a| a.name.ends_with(suffix))?;
    Some(Update {
        version: release.tag_name.trim_start_matches('v').to_string(),
        asset_name: asset.name.clone(),
        asset_url: asset.browser_download_url.clone(),
    })
}

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

/// Streams `url` to `to`, counting into `progress` as it goes.
async fn download(url: &str, to: &Path, progress: &Progress) -> Result<()> {
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
    while let Some(chunk) = response.chunk().await.context("download interrupted")? {
        file.write_all(&chunk)
            .with_context(|| format!("couldn't write {}", to.display()))?;
        progress
            .received
            .fetch_add(chunk.len() as u64, Ordering::Relaxed);
    }
    file.flush()
        .with_context(|| format!("couldn't write {}", to.display()))
}

async fn apply_archive(root: &Path, update: &Update, progress: &Progress) -> Result<()> {
    // Staged inside the install so the final renames never cross filesystems.
    let staging = root.join(".update");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let archive = staging.join("update.tar.gz");
    download(&update.asset_url, &archive, progress).await?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .context("couldn't run tar")?;
    if !status.success() {
        bail!("couldn't unpack {}", update.asset_name);
    }
    let unpacked = find_bundle(&staging).context("the update archive has no bin/embyclientplus")?;
    for part in ["bin", "lib"] {
        let old = staging.join(format!("old-{part}"));
        let current = root.join(part);
        if current.exists() {
            std::fs::rename(&current, &old)?;
        }
        if let Err(e) = std::fs::rename(unpacked.join(part), &current) {
            // Put the old copy back rather than leave a broken install.
            let _ = std::fs::rename(&old, &current);
            return Err(e).context("couldn't install the update");
        }
    }
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

/// The folder in the unpacked archive holding bin/ and lib/ (the archive
/// wraps them in a versioned folder).
fn find_bundle(staging: &Path) -> Option<PathBuf> {
    let has_bundle = |dir: &Path| dir.join("bin/embyclientplus-bin").is_file();
    if has_bundle(staging) {
        return Some(staging.to_path_buf());
    }
    std::fs::read_dir(staging)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|dir| has_bundle(dir))
}

async fn apply_appimage(path: &Path, update: &Update, progress: &Progress) -> Result<()> {
    let partial = path.with_extension("AppImage.part");
    download(&update.asset_url, &partial, progress).await?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&partial, path).context("couldn't replace the AppImage")
}

/// Starts the updated copy; the caller then quits.
pub fn relaunch(kind: &InstallKind) -> Result<()> {
    let program = match kind {
        InstallKind::Archive(root) => root.join("bin/embyclientplus"),
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
                },
                AssetJson {
                    name: format!("EmbyClientPlus-{tag}-x86_64.AppImage"),
                    browser_download_url: "https://x/appimage".into(),
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
        assert!(update.asset_name.ends_with("-linux-x86_64.tar.gz"));
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
        std::fs::write(bundle.join("bin/embyclientplus-bin"), b"").unwrap();
        assert_eq!(find_bundle(&root), Some(bundle));
        let _ = std::fs::remove_dir_all(&root);
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
            .block_on(download(&url, &to, &progress))
            .unwrap();
        let size = std::fs::metadata(&to).unwrap().len();
        assert_eq!(progress.received(), size);
        assert_eq!(progress.total(), Some(size));
        assert_eq!(progress.fraction(), Some(1.0));
        let _ = std::fs::remove_file(&to);
    }
}
