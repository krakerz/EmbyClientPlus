//! Using the user's own mpv configuration (`~/.config/mpv`, or
//! `%APPDATA%\mpv` on Windows): shaders,
//! scalers, subtitle styling and so on. mpv reads it at start-up, so a
//! filtered copy is staged in a private config dir: options that would
//! break embedding or override the app's own control are dropped.

use std::path::{Path, PathBuf};

/// Options the app owns, or that would break rendering into our widget.
const DROPPED: [&str; 12] = [
    "input-ipc-server",
    "vo",
    "wid",
    "gpu-context",
    "gpu-api",
    "terminal",
    "idle",
    "force-window",
    "input-default-bindings",
    "osc",
    "fs",
    "fullscreen",
];

/// Subfolders `~~/...` paths in mpv.conf may refer to.
const LINKED: [&str; 5] = ["shaders", "script-opts", "scripts", "fonts", "scripts-opts"];

/// The user's mpv config dir, if it has an mpv.conf. mpv itself uses
/// `%APPDATA%\mpv` on Windows and `~/.config/mpv` everywhere else (macOS
/// included), honouring `$XDG_CONFIG_HOME`.
pub fn source_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        directories::BaseDirs::new().map(|dirs| dirs.config_dir().to_path_buf())
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".config")))
    }?;
    let dir = base.join("mpv");
    dir.join("mpv.conf").is_file().then_some(dir)
}

/// Builds the filtered config dir in `staging` and returns it. `keep_hwdec`
/// is false while SVP is the default, since SVP needs copy-back decoding.
pub fn stage(source: &Path, staging: &Path, keep_hwdec: bool) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(staging)?;
    let original = std::fs::read_to_string(source.join("mpv.conf"))?;
    let (filtered, dropped) = filter(&original, keep_hwdec);
    for line in dropped {
        tracing::info!("mpv.conf: ignoring `{line}` (the app manages it)");
    }
    std::fs::write(staging.join("mpv.conf"), filtered)?;
    for name in LINKED {
        let link = staging.join(name);
        let target = source.join(name);
        link_dir(&target, &link)?;
    }
    Ok(staging.to_path_buf())
}

/// Makes `link` show `target` (if it exists): a symlink on Unix, a copy on
/// Windows, where symlinks need developer mode or admin rights.
#[cfg(unix)]
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(link);
    if target.exists() {
        std::os::unix::fs::symlink(target, link)?;
    }
    Ok(())
}

#[cfg(windows)]
fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_dir_all(link);
    if target.is_dir() {
        copy_dir(target, link)?;
    }
    Ok(())
}

#[cfg(windows)]
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// The config text without the app-managed options, plus what was dropped.
fn filter(conf: &str, keep_hwdec: bool) -> (String, Vec<String>) {
    let mut kept = String::with_capacity(conf.len());
    let mut dropped = Vec::new();
    for line in conf.lines() {
        if managed(line, keep_hwdec) {
            dropped.push(line.trim().to_string());
            kept.push_str("# (ignored by EmbyClientPlus) ");
        }
        kept.push_str(line);
        kept.push('\n');
    }
    (kept, dropped)
}

fn managed(line: &str, keep_hwdec: bool) -> bool {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
        return false;
    }
    let key = line.split(['=', ' ', '\t']).next().unwrap_or_default();
    let key = key.trim_start_matches("--");
    let key = key.strip_prefix("no-").unwrap_or(key);
    DROPPED.contains(&key) || (!keep_hwdec && key == "hwdec")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_managed_options_are_commented_out() {
        let conf = "\
# my config
profile=gpu-hq
glsl-shaders=~~/shaders/FSRCNNX.glsl
input-ipc-server=/tmp/other
vo=gpu-next
hwdec=vaapi
no-osc
[svp]
fs=yes
sub-font-size=40
";
        let (filtered, dropped) = filter(conf, false);
        assert_eq!(
            dropped,
            [
                "input-ipc-server=/tmp/other",
                "vo=gpu-next",
                "hwdec=vaapi",
                "no-osc",
                "fs=yes"
            ]
        );
        assert!(filtered.contains("\nglsl-shaders=~~/shaders/FSRCNNX.glsl\n"));
        assert!(filtered.contains("# (ignored by EmbyClientPlus) vo=gpu-next"));
        assert!(filtered.contains("\n[svp]\n"));
        assert!(filtered.contains("\nsub-font-size=40\n"));
        // With SVP off by default, the user's hwdec choice stands.
        let (_, dropped) = filter(conf, true);
        assert!(!dropped.iter().any(|line| line.starts_with("hwdec")));
    }

    #[test]
    fn staging_links_shader_folders() {
        let root =
            std::env::temp_dir().join(format!("embyclientplus-mpvconf-{}", std::process::id()));
        let source = root.join("mpv");
        std::fs::create_dir_all(source.join("shaders")).unwrap();
        std::fs::write(source.join("mpv.conf"), "vo=gpu\nscale=ewa_lanczossharp\n").unwrap();
        let staged = stage(&source, &root.join("staged"), true).unwrap();
        let conf = std::fs::read_to_string(staged.join("mpv.conf")).unwrap();
        assert!(conf.contains("# (ignored by EmbyClientPlus) vo=gpu"));
        assert!(staged.join("shaders").is_symlink());
        assert!(!staged.join("fonts").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
