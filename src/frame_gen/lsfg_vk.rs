use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::config_dir;

/// Distinct process name registered in lsfg-vk's own `active_in` list, so
/// the layer only ever hooks playback launched through us — never the
/// user's own unrelated mpv usage. `/proc/pid/comm` truncates at 15
/// chars, but lsfg-vk itself reads the untruncated executable path (real
/// user profiles in the wild have longer names than that and still
/// match), confirmed indirectly — see NOTES.md.
pub const PROCESS_NAME: &str = "embyclientplus-mpv";

fn conf_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".config/lsfg-vk/conf.toml"))
}

/// Ensures our own profile is registered in lsfg-vk's config, without
/// touching any existing profile a user already has for their own games.
/// Purely additive: does nothing if we're already registered, otherwise
/// appends one new `[[profile]]` table.
///
/// Returns an error if lsfg-vk isn't installed/configured at all (no
/// `conf.toml`) — callers should treat that as "lsfg-vk mode
/// unavailable", not attempt to create the file from scratch, since the
/// required `[global] dll` path is the user's own Lossless Scaling
/// install location, not something we can guess.
pub fn ensure_active_in_registered(multiplier: u32, flow_scale: f64) -> Result<()> {
    ensure_active_in_registered_at(&conf_path()?, multiplier, flow_scale)
}

fn ensure_active_in_registered_at(path: &Path, multiplier: u32, flow_scale: f64) -> Result<()> {
    if !path.exists() {
        bail!(
            "lsfg-vk is not configured ({} does not exist) — lsfg-vk mode unavailable",
            path.display()
        );
    }
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let mut doc: toml::Value =
        toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))?;

    let table = doc
        .as_table_mut()
        .context("lsfg-vk conf.toml is not a table at its root")?;
    let profiles = table
        .entry("profile")
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .context("lsfg-vk conf.toml's `profile` key is not an array")?;

    let already_registered = profiles.iter().any(|profile| {
        profile
            .get("active_in")
            .and_then(|v| v.as_array())
            .is_some_and(|active_in| active_in.iter().any(|v| v.as_str() == Some(PROCESS_NAME)))
    });
    if already_registered {
        return Ok(());
    }

    let mut new_profile = toml::map::Map::new();
    new_profile.insert(
        "active_in".to_string(),
        toml::Value::Array(vec![toml::Value::String(PROCESS_NAME.to_string())]),
    );
    new_profile.insert("flow_scale".to_string(), toml::Value::Float(flow_scale));
    new_profile.insert(
        "multiplier".to_string(),
        toml::Value::Integer(multiplier as i64),
    );
    new_profile.insert(
        "name".to_string(),
        toml::Value::String("EmbyClientPlus".to_string()),
    );
    new_profile.insert(
        "override_present_mode".to_string(),
        toml::Value::Boolean(true),
    );
    new_profile.insert(
        "pacing_mode".to_string(),
        toml::Value::String("vsync".to_string()),
    );
    new_profile.insert("performance_mode".to_string(), toml::Value::Boolean(false));
    new_profile.insert(
        "preserve_swapchain_image_count".to_string(),
        toml::Value::Boolean(true),
    );
    profiles.push(toml::Value::Table(new_profile));

    let serialized =
        toml::to_string_pretty(&doc).context("failed to serialize lsfg-vk conf.toml")?;
    std::fs::write(path, serialized).with_context(|| format!("failed to write {}", path.display()))
}

/// Creates (or verifies) a symlink at a stable path named [`PROCESS_NAME`]
/// pointing at the real `mpv` binary, so launching through it registers
/// as our own distinct process for lsfg-vk's `active_in` matching.
pub fn ensure_process_symlink(real_mpv: &Path) -> Result<PathBuf> {
    let symlink_path = config_dir()?.join(PROCESS_NAME);
    if let Some(parent) = symlink_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let needs_recreate = match std::fs::read_link(&symlink_path) {
        Ok(target) => target != real_mpv,
        Err(_) => true,
    };
    if needs_recreate {
        let _ = std::fs::remove_file(&symlink_path);
        std::os::unix::fs::symlink(real_mpv, &symlink_path)
            .with_context(|| format!("failed to symlink {}", symlink_path.display()))?;
    }
    Ok(symlink_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_conf_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "embyclientplus-lsfgvk-test-{}-{}",
            std::process::id(),
            name
        ))
    }

    #[test]
    fn missing_file_is_an_error_not_a_creation() {
        let path = temp_conf_path("missing.toml");
        let result = ensure_active_in_registered_at(&path, 2, 1.0);
        assert!(result.is_err());
        assert!(!path.exists());
    }

    #[test]
    fn appends_a_new_profile_without_touching_existing_ones() {
        let path = temp_conf_path("existing.toml");
        std::fs::write(
            &path,
            r#"
version = 2

[global]
allow_fp16 = true
dll = "/home/user/.steam/steam/steamapps/common/Lossless Scaling/lsfg-vk.dll"
log_level = "info"

[[profile]]
active_in = ["somegame.exe"]
flow_scale = 1.0
multiplier = 2
name = "existing profile"
override_present_mode = true
pacing_mode = "vsync"
performance_mode = false
preserve_swapchain_image_count = true
"#,
        )
        .unwrap();

        ensure_active_in_registered_at(&path, 3, 0.8).unwrap();

        let doc: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let profiles = doc["profile"].as_array().unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0]["active_in"][0].as_str(), Some("somegame.exe"));
        assert_eq!(profiles[1]["active_in"][0].as_str(), Some(PROCESS_NAME));
        assert_eq!(profiles[1]["multiplier"].as_integer(), Some(3));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn is_idempotent_when_already_registered() {
        let path = temp_conf_path("idempotent.toml");
        std::fs::write(
            &path,
            r#"
version = 2

[global]
dll = "x"

[[profile]]
active_in = ["embyclientplus-mpv"]
multiplier = 2
"#,
        )
        .unwrap();

        ensure_active_in_registered_at(&path, 4, 1.0).unwrap();

        let doc: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let profiles = doc["profile"].as_array().unwrap();
        assert_eq!(profiles.len(), 1, "must not duplicate an existing entry");

        let _ = std::fs::remove_file(&path);
    }
}
