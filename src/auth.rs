use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::config::config_dir;

const SERVICE: &str = "embyclientplus";

/// Stores the Emby access token for a given server, preferring the
/// system keyring (secret-service over D-Bus) and falling back to a
/// plaintext file only when no keyring backend is reachable (e.g. a bare
/// window-manager setup with no secret-service daemon running).
pub fn store_token(server_url: &str, token: &str) -> Result<()> {
    match Entry::new(SERVICE, server_url).and_then(|entry| entry.set_password(token)) {
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
    match Entry::new(SERVICE, server_url).and_then(|entry| entry.get_password()) {
        Ok(token) => Ok(Some(token)),
        Err(_) => get_plaintext_token(server_url),
    }
}

pub fn delete_token(server_url: &str) -> Result<()> {
    let _ = Entry::new(SERVICE, server_url).and_then(|entry| entry.delete_credential());
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
    std::fs::write(path, contents)
        .with_context(|| format!("failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to restrict permissions on {}", path.display()))?;
    }
    Ok(())
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
