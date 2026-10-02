//! Whether SVP is installed and SVP Manager is running, so the player can
//! say why interpolation isn't happening instead of silently not doing it.

use std::path::PathBuf;

/// SVP's Linux installer always uses this layout.
fn install_dir() -> Option<PathBuf> {
    std::env::var_os("SVP_DIR")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join("SVP4")))
}

pub fn installed() -> bool {
    install_dir().is_some_and(|dir| dir.join("SVPManager").exists())
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
