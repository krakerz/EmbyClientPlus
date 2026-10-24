use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::config::config_dir;

const SERVICE: &str = "embyclientplus";

/// Runs `f` on a fresh plain thread and waits for it. The secret-service
/// keyring store drives D-Bus with its own `block_on`, which panics when it
/// is reached from inside a tokio runtime (and poisons the store's mutex, so
/// every later keyring call would panic too). A new thread has no runtime
/// context, so keyring calls stay safe from tokio tasks. Not `spawn_blocking`:
/// pool threads still carry the runtime handle.
fn off_runtime<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| match scope.spawn(f).join() {
        Ok(value) => value,
        Err(panic) => std::panic::resume_unwind(panic),
    })
}

/// The keyring operations the token store needs, so the policy below can be
/// tested without a real Secret Service.
trait Keyring {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, token: &str) -> bool;
    fn delete(&self, key: &str);
}

/// The system keyring; every call runs through `off_runtime`.
struct SystemKeyring;

impl Keyring for SystemKeyring {
    fn get(&self, key: &str) -> Option<String> {
        off_runtime(|| Entry::new(SERVICE, key).and_then(|entry| entry.get_password())).ok()
    }
    fn set(&self, key: &str, token: &str) -> bool {
        off_runtime(|| Entry::new(SERVICE, key).and_then(|entry| entry.set_password(token))).is_ok()
    }
    fn delete(&self, key: &str) {
        let _ =
            off_runtime(|| Entry::new(SERVICE, key).and_then(|entry| entry.delete_credential()));
    }
}

/// Whether `/etc/os-release` names a gaming-handheld OS (SteamOS, Bazzite,
/// ChimeraOS, HoloISO, or a Deck variant).
fn os_release_uses_shared_file(text: &str) -> bool {
    const SHARED: [&str; 5] = ["steamos", "holo", "bazzite", "chimeraos", "holoiso"];
    let mut hit = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .to_lowercase();
        match key.trim() {
            "ID" | "ID_LIKE" => {
                hit |= value.split_whitespace().any(|id| SHARED.contains(&id));
            }
            "VARIANT_ID" => hit |= value.contains("deck") || value.contains("handheld"),
            _ => {}
        }
    }
    hit
}

/// Whether a gamescope Game Mode session is installed: a `steamos-session-select`
/// or `gamescope-session*` binary on PATH (`on_path`), or a `gamescope-session*`
/// desktop-session file name (`session_files`, from the wayland/x11 session dirs).
fn game_mode_installed(on_path: impl Fn(&str) -> bool, session_files: &[String]) -> bool {
    [
        "steamos-session-select",
        "gamescope-session",
        "gamescope-session-plus",
    ]
    .iter()
    .any(|name| on_path(name))
        || session_files
            .iter()
            .any(|name| name.starts_with("gamescope-session"))
}

fn game_mode_installed_here() -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    let on_path = |name: &str| {
        dirs.iter().any(|dir| dir.join(name).is_file())
            || std::path::Path::new("/usr/bin").join(name).is_file()
    };
    let mut files = Vec::new();
    for dir in ["/usr/share/wayland-sessions", "/usr/share/xsessions"] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            files.extend(
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned()),
            );
        }
    }
    game_mode_installed(on_path, &files)
}

/// Whether tokens live in the private file in every mode. Steam Deck style
/// systems run a keyring only in Desktop Mode, so Game Mode and Desktop Mode
/// would otherwise keep separate logins. On Linux the file is used when a
/// Game Mode session is installed, the app runs in Game Mode now (remembered
/// in a marker file for later Desktop Mode runs), or os-release names a
/// handheld OS. `EMBYCLIENTPLUS_TOKEN_STORE=file|keyring` overrides all.
fn use_shared_file() -> bool {
    static SHARED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SHARED.get_or_init(|| {
        match std::env::var("EMBYCLIENTPLUS_TOKEN_STORE").as_deref() {
            Ok("file") => return true,
            Ok("keyring") => return false,
            _ => {}
        }
        if !cfg!(target_os = "linux") {
            return false;
        }
        let marker = config_dir().ok().map(|dir| dir.join("game_mode_seen"));
        if crate::gamescope::detected() {
            if let Some(marker) = &marker {
                if let Some(parent) = marker.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(marker, b"");
            }
            return true;
        }
        marker.is_some_and(|marker| marker.exists())
            || game_mode_installed_here()
            || std::fs::read_to_string("/etc/os-release")
                .map(|text| os_release_uses_shared_file(&text))
                .unwrap_or(false)
    })
}

/// True on systems where the private file is the shared token store (the
/// UI uses it to word the Game Mode login hint).
pub fn tokens_shared_across_modes() -> bool {
    use_shared_file()
}

/// Stores the Emby access token for a given server, preferring the
/// system keyring (secret-service over D-Bus) and falling back to a
/// plaintext file only when no keyring backend is reachable (e.g. a bare
/// window-manager setup with no secret-service daemon running). On
/// shared-file systems (see `use_shared_file`) it always uses the file.
pub fn store_token(server_url: &str, token: &str) -> Result<()> {
    store_token_with(
        &plaintext_store_path()?,
        &SystemKeyring,
        use_shared_file(),
        server_url,
        token,
    )
}

pub fn get_token(server_url: &str) -> Result<Option<String>> {
    get_token_with(
        &plaintext_store_path()?,
        &SystemKeyring,
        use_shared_file(),
        server_url,
    )
}

pub fn delete_token(server_url: &str) -> Result<()> {
    delete_token_with(&plaintext_store_path()?, &SystemKeyring, server_url)
}

fn store_token_with(
    path: &std::path::Path,
    keyring: &dyn Keyring,
    shared_file: bool,
    server_url: &str,
    token: &str,
) -> Result<()> {
    if shared_file {
        store_plaintext_token_at(path, server_url, token)?;
        // One truth: drop any keyring copy so it can't go stale.
        keyring.delete(server_url);
        return Ok(());
    }
    if keyring.set(server_url, token) {
        // Keyring succeeded — make sure a stale plaintext copy from an
        // earlier no-keyring run doesn't linger and shadow it.
        let _ = remove_plaintext_token_at(path, server_url);
        Ok(())
    } else {
        store_plaintext_token_at(path, server_url, token)
    }
}

fn get_token_with(
    path: &std::path::Path,
    keyring: &dyn Keyring,
    shared_file: bool,
    server_url: &str,
) -> Result<Option<String>> {
    if shared_file {
        if let Some(token) = get_plaintext_token_at(path, server_url)? {
            return Ok(Some(token));
        }
        // Migrate a login made in Desktop Mode (keyring) into the file.
        return match keyring.get(server_url) {
            Some(token) => {
                let _ = store_plaintext_token_at(path, server_url, &token);
                Ok(Some(token))
            }
            None => Ok(None),
        };
    }
    match keyring.get(server_url) {
        Some(token) => Ok(Some(token)),
        None => get_plaintext_token_at(path, server_url),
    }
}

fn delete_token_with(
    path: &std::path::Path,
    keyring: &dyn Keyring,
    server_url: &str,
) -> Result<()> {
    keyring.delete(server_url);
    remove_plaintext_token_at(path, server_url)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PlaintextStore {
    #[serde(default)]
    tokens: HashMap<String, String>,
}

fn plaintext_store_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("credentials-fallback.toml"))
}

fn load_plaintext_store_at(path: &std::path::Path) -> Result<PlaintextStore> {
    if !path.exists() {
        return Ok(PlaintextStore::default());
    }
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))
}

fn save_plaintext_store_at(path: &std::path::Path, store: &PlaintextStore) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let contents = toml::to_string_pretty(store).context("failed to serialize credentials")?;
    write_private(path, contents.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))
}

/// Writes `contents` atomically, never readable by others: on unix the
/// temporary file is created 0o600, so there's no window where the tokens
/// are world-readable.
fn write_private(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".tmp");
        let temp = path.with_file_name(name);
        // A leftover temp file could carry looser permissions.
        let _ = std::fs::remove_file(&temp);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    }
    #[cfg(not(unix))]
    crate::config::write_atomic(path, contents)
}

fn store_plaintext_token_at(path: &std::path::Path, server_url: &str, token: &str) -> Result<()> {
    let mut store = load_plaintext_store_at(path)?;
    store
        .tokens
        .insert(server_url.to_string(), token.to_string());
    save_plaintext_store_at(path, &store)
}

fn get_plaintext_token_at(path: &std::path::Path, server_url: &str) -> Result<Option<String>> {
    let store = load_plaintext_store_at(path)?;
    Ok(store.tokens.get(server_url).cloned())
}

fn remove_plaintext_token_at(path: &std::path::Path, server_url: &str) -> Result<()> {
    let mut store = load_plaintext_store_at(path)?;
    if store.tokens.remove(server_url).is_some() {
        save_plaintext_store_at(path, &store)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn off_runtime_allows_a_nested_block_on() {
        // Mirrors the keyring store: it calls `block_on` itself, which
        // panics inside a tokio runtime unless it runs on a plain thread.
        let value = off_runtime(|| {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async { 40 + 2 })
        });
        assert_eq!(value, 42);
    }

    #[test]
    fn plaintext_store_round_trips_and_restricts_permissions() {
        let dir =
            std::env::temp_dir().join(format!("embyclientplus-auth-test-{}", std::process::id()));
        let path = dir.join("credentials-fallback.toml");

        store_plaintext_token_at(&path, "http://server-a", "token-a").unwrap();
        store_plaintext_token_at(&path, "http://server-b", "token-b").unwrap();

        assert_eq!(
            get_plaintext_token_at(&path, "http://server-a").unwrap(),
            Some("token-a".to_string())
        );
        assert_eq!(
            get_plaintext_token_at(&path, "http://server-b").unwrap(),
            Some("token-b".to_string())
        );
        assert_eq!(
            get_plaintext_token_at(&path, "http://unknown").unwrap(),
            None
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        remove_plaintext_token_at(&path, "http://server-a").unwrap();
        assert_eq!(
            get_plaintext_token_at(&path, "http://server-a").unwrap(),
            None
        );
        assert_eq!(
            get_plaintext_token_at(&path, "http://server-b").unwrap(),
            Some("token-b".to_string())
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeKeyring {
        map: RefCell<HashMap<String, String>>,
        broken: bool,
    }

    impl Keyring for FakeKeyring {
        fn get(&self, key: &str) -> Option<String> {
            self.map.borrow().get(key).cloned()
        }
        fn set(&self, key: &str, token: &str) -> bool {
            if self.broken {
                return false;
            }
            self.map.borrow_mut().insert(key.into(), token.into());
            true
        }
        fn delete(&self, key: &str) {
            self.map.borrow_mut().remove(key);
        }
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("embyclientplus-auth-{tag}-{}", std::process::id()))
            .join("credentials-fallback.toml")
    }

    #[test]
    fn game_mode_session_detection() {
        let none: Vec<String> = vec![];
        assert!(!game_mode_installed(|_| false, &none));
        assert!(!game_mode_installed(
            |_| false,
            &["plasma.desktop".to_string(), "gnome.desktop".to_string()]
        ));
        assert!(game_mode_installed(
            |n| n == "steamos-session-select",
            &none
        ));
        assert!(game_mode_installed(
            |n| n == "gamescope-session-plus",
            &none
        ));
        assert!(game_mode_installed(
            |_| false,
            &[
                "plasma.desktop".to_string(),
                "gamescope-session.desktop".to_string()
            ]
        ));
        assert!(game_mode_installed(
            |_| false,
            &["gamescope-session-steam.desktop".to_string()]
        ));
    }

    #[test]
    fn os_release_detection() {
        let steamos = "NAME=\"SteamOS\"\nID=steamos\nID_LIKE=arch\nVARIANT_ID=steamdeck\n";
        let holo = "ID=arch\nID_LIKE=\"holo\"\n";
        let bazzite = "NAME=\"Bazzite\"\nID=bazzite\nID_LIKE=\"fedora\"\nVARIANT_ID=steam\n";
        let chimera = "ID=chimeraos\n";
        let holoiso = "ID=holoiso\n";
        let deck_variant = "ID=fedora\nVARIANT_ID=\"deck-gnome\"\n";
        let arch = "NAME=\"Arch Linux\"\nID=arch\n";
        let cachy = "ID=cachyos\nID_LIKE=arch\n";
        let handheld = "ID=cachyos\nVARIANT_ID=handheld\n";
        let ubuntu = "ID=ubuntu\nID_LIKE=debian\nVARIANT_ID=desktop\n";
        for text in [
            steamos,
            holo,
            bazzite,
            chimera,
            holoiso,
            deck_variant,
            handheld,
        ] {
            assert!(os_release_uses_shared_file(text), "{text}");
        }
        for text in [arch, cachy, ubuntu, ""] {
            assert!(!os_release_uses_shared_file(text), "{text}");
        }
    }

    #[test]
    fn shared_file_store_writes_file_and_clears_keyring() {
        let path = temp_path("store");
        let kr = FakeKeyring::default();
        kr.set("s", "old");
        store_token_with(&path, &kr, true, "s", "new").unwrap();
        assert_eq!(kr.get("s"), None);
        assert_eq!(
            get_plaintext_token_at(&path, "s").unwrap(),
            Some("new".into())
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn shared_file_get_prefers_file_then_migrates_keyring() {
        let path = temp_path("get");
        let kr = FakeKeyring::default();
        kr.set("s", "from-keyring");
        assert_eq!(
            get_token_with(&path, &kr, true, "s").unwrap(),
            Some("from-keyring".into())
        );
        assert_eq!(
            get_plaintext_token_at(&path, "s").unwrap(),
            Some("from-keyring".into())
        );
        store_plaintext_token_at(&path, "s", "from-file").unwrap();
        assert_eq!(
            get_token_with(&path, &kr, true, "s").unwrap(),
            Some("from-file".into())
        );
        assert_eq!(get_token_with(&path, &kr, true, "none").unwrap(), None);
        delete_token_with(&path, &kr, "s").unwrap();
        assert_eq!(kr.get("s"), None);
        assert_eq!(get_plaintext_token_at(&path, "s").unwrap(), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn default_mode_prefers_keyring_and_falls_back_to_file() {
        let path = temp_path("default");
        let kr = FakeKeyring::default();
        store_plaintext_token_at(&path, "s", "stale").unwrap();
        store_token_with(&path, &kr, false, "s", "tok").unwrap();
        assert_eq!(kr.get("s"), Some("tok".into()));
        assert_eq!(get_plaintext_token_at(&path, "s").unwrap(), None);
        assert_eq!(
            get_token_with(&path, &kr, false, "s").unwrap(),
            Some("tok".into())
        );

        let broken = FakeKeyring {
            broken: true,
            ..Default::default()
        };
        store_token_with(&path, &broken, false, "t", "ft").unwrap();
        assert_eq!(
            get_token_with(&path, &broken, false, "t").unwrap(),
            Some("ft".into())
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
