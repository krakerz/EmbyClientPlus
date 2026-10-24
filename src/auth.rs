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

/// Stores the Emby access token for a given server, preferring the
/// system keyring (secret-service over D-Bus) and falling back to a
/// plaintext file only when no keyring backend is reachable (e.g. a bare
/// window-manager setup with no secret-service daemon running).
pub fn store_token(server_url: &str, token: &str) -> Result<()> {
    let stored =
        off_runtime(|| Entry::new(SERVICE, server_url).and_then(|entry| entry.set_password(token)));
    match stored {
        Ok(()) => {
            // Keyring succeeded — make sure a stale plaintext copy from an
            // earlier no-keyring run doesn't linger and shadow it.
            let _ = remove_plaintext_token(server_url);
            Ok(())
        }
        Err(_) => store_plaintext_token(server_url, token),
    }
}

pub fn get_token(server_url: &str) -> Result<Option<String>> {
    match off_runtime(|| Entry::new(SERVICE, server_url).and_then(|entry| entry.get_password())) {
        Ok(token) => Ok(Some(token)),
        Err(_) => get_plaintext_token(server_url),
    }
}

pub fn delete_token(server_url: &str) -> Result<()> {
    let _ =
        off_runtime(|| Entry::new(SERVICE, server_url).and_then(|entry| entry.delete_credential()));
    remove_plaintext_token(server_url)
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

fn store_plaintext_token(server_url: &str, token: &str) -> Result<()> {
    store_plaintext_token_at(&plaintext_store_path()?, server_url, token)
}

fn get_plaintext_token(server_url: &str) -> Result<Option<String>> {
    get_plaintext_token_at(&plaintext_store_path()?, server_url)
}

fn remove_plaintext_token(server_url: &str) -> Result<()> {
    remove_plaintext_token_at(&plaintext_store_path()?, server_url)
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
}
