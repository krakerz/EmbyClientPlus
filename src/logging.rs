//! Logging to stderr and to a per-launch file in the config directory,
//! keeping only the newest few files. Panics are logged too, since a panic
//! inside a GTK callback aborts before stderr is usually captured.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use crate::config::config_dir;

/// Log files kept, this launch's included.
const KEEP_LOGS: usize = 10;
const PREFIX: &str = "embyclientplus-";
const SUFFIX: &str = ".log";

/// Sets up logging; returns the log file's path when file logging works
/// (stderr logging works regardless).
pub fn init() -> Option<PathBuf> {
    let filter = || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let file = open_log_file();
    let file_layer = file.as_ref().ok().map(|(file, _)| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(Mutex::new(file.try_clone().expect("log file handle")))
            .with_filter(filter())
    });
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter()))
        .with(file_layer)
        .init();

    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("panic: {info}\n{backtrace}");
    }));

    match file {
        Ok((_, path)) => {
            tracing::info!("logging to {}", path.display());
            Some(path)
        }
        Err(e) => {
            tracing::warn!("file logging disabled: {e:#}");
            None
        }
    }
}

/// Where log files go.
pub fn log_dir() -> Option<PathBuf> {
    config_dir().ok().map(|dir| dir.join("logs"))
}

fn open_log_file() -> Result<(File, PathBuf)> {
    let dir = log_dir().context("no config directory")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = dir.join(format!("{PREFIX}{}{SUFFIX}", timestamp(SystemTime::now())));
    let file =
        File::create(&path).with_context(|| format!("failed to create {}", path.display()))?;
    prune(&dir, KEEP_LOGS);
    Ok((file, path))
}

/// Deletes all but the newest `keep` log files. Names sort by time.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(PREFIX) && name.ends_with(SUFFIX))
        })
        .collect();
    logs.sort();
    let excess = logs.len().saturating_sub(keep);
    for old in &logs[..excess] {
        let _ = std::fs::remove_file(old);
    }
}

/// `YYYYMMDD-HHMMSS` in UTC; sorts chronologically as text.
fn timestamp(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_utc_and_sortable() {
        assert_eq!(timestamp(UNIX_EPOCH), "19700101-000000");
        // 2026-10-02 09:38:41 UTC
        let time = UNIX_EPOCH + Duration::from_secs(1_790_933_921);
        assert_eq!(timestamp(time), "20261002-093841");
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn prune_keeps_the_newest_logs_only() {
        let dir = std::env::temp_dir().join(format!("embyclientplus-logs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for stamp in ["20260101-000000", "20260102-000000", "20260103-000000"] {
            File::create(dir.join(format!("{PREFIX}{stamp}{SUFFIX}"))).unwrap();
        }
        File::create(dir.join("unrelated.txt")).unwrap();
        prune(&dir, 2);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "embyclientplus-20260102-000000.log",
                "embyclientplus-20260103-000000.log",
                "unrelated.txt"
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
