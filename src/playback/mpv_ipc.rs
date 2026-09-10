use std::path::Path;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use mpvipc::Mpv;

/// mpv needs a moment after spawning to create and start listening on
/// the IPC socket — confirmed in the M2 spike that a fixed retry loop is
/// necessary (a bare connect attempt immediately after spawn reliably
/// fails).
pub fn connect_with_retry(socket_path: &Path) -> Result<Mpv> {
    let mut last_error = None;
    for _ in 0..50 {
        match Mpv::connect(&socket_path.to_string_lossy()) {
            Ok(mpv) => return Ok(mpv),
            Err(e) => last_error = Some(e),
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!(
        "failed to connect to mpv's IPC socket at {} after 5s: {:?}",
        socket_path.display(),
        last_error
    );
}

pub fn current_position_ticks(mpv: &Mpv) -> Result<i64> {
    let seconds: f64 = mpv
        .get_property("time-pos")
        .context("failed to read mpv time-pos")?;
    // Emby positions are in 100-nanosecond ticks.
    Ok((seconds * 10_000_000.0) as i64)
}

pub fn is_paused(mpv: &Mpv) -> Result<bool> {
    mpv.get_property("pause")
        .context("failed to read mpv pause state")
}
