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

/// The playing file's total duration in seconds, for the seek bar's fill
/// fraction. Can transiently fail right after a (re)spawn before mpv has
/// finished probing the file — callers already run this every tick, so a
/// momentary error just skips drawing the seek bar that tick and
/// self-heals on the next one; no retry wrapper needed here.
pub fn duration_seconds(mpv: &Mpv) -> Result<f64> {
    mpv.get_property("duration")
        .context("failed to read mpv duration")
}

pub fn seek_to(mpv: &Mpv, seconds: f64) -> Result<()> {
    mpv.set_property("time-pos", seconds)
        .context("failed to seek mpv")
}

/// The audio tracks mpv itself demuxed from the currently-playing file
/// (id + language) — direct-played/streamed containers keep every
/// embedded audio track, so switching is a matter of asking mpv which
/// ones exist and telling it which `aid` to use, no Emby involvement
/// needed. mpvipc's typed `get_property` only supports a handful of
/// concrete types (no generic JSON value), so this walks mpv's own
/// `track-list/N/...` indexed sub-properties instead of the compound
/// `track-list` property directly.
pub fn audio_tracks(mpv: &Mpv) -> Result<Vec<(i64, String)>> {
    let count: usize = mpv
        .get_property("track-list/count")
        .context("failed to read mpv track-list/count")?;
    let mut tracks = Vec::new();
    for i in 0..count {
        let track_type: String = mpv
            .get_property(&format!("track-list/{i}/type"))
            .with_context(|| format!("failed to read mpv track-list/{i}/type"))?;
        if track_type != "audio" {
            continue;
        }
        let id: i64 = mpv
            .get_property(&format!("track-list/{i}/id"))
            .with_context(|| format!("failed to read mpv track-list/{i}/id"))?;
        let lang: String = mpv
            .get_property(&format!("track-list/{i}/lang"))
            .unwrap_or_else(|_| "und".to_string());
        tracks.push((id, lang));
    }
    Ok(tracks)
}

pub fn set_audio_track(mpv: &Mpv, id: i64) -> Result<()> {
    mpv.set_property("aid", id)
        .context("failed to set mpv audio track")
}

/// Same as [`audio_tracks`], but tolerates the same startup delay
/// `connect_with_retry` does — right after the IPC socket connects, mpv
/// hasn't necessarily finished demuxing the file yet, so `track-list`
/// can genuinely (successfully) report zero tracks for a moment even
/// when the file has audio. Confirmed live: querying immediately after
/// connect returned an empty list for a title mpv's own log showed had
/// exactly one audio track.
pub fn audio_tracks_with_retry(mpv: &Mpv) -> Result<Vec<(i64, String)>> {
    for attempt in 0..30 {
        let tracks = audio_tracks(mpv)?;
        if !tracks.is_empty() || attempt == 29 {
            return Ok(tracks);
        }
        thread::sleep(Duration::from_millis(100));
    }
    unreachable!()
}
