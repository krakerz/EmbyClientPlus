//! Playback timing diagnostics and env test switches.
//!
//! Env switches (read once at startup, logged when set; for diagnosing
//! timing problems such as dropped frames under gamescope):
//! - `EMBYCLIENTPLUS_VIDEO_SYNC=<mode>`: replaces the mpv `video-sync` the
//!   app would choose (e.g. `audio`, `display-resample`).
//! - `EMBYCLIENTPLUS_DISPLAY_FPS=<number>|off`: a number replaces the
//!   `display-fps-override` the Preferences setting would give; `off` sets
//!   none. Wins over the setting.
//! - `EMBYCLIENTPLUS_REPORT_SWAP=0`: stops telling mpv when frames went out.
//! - `EMBYCLIENTPLUS_STATS=1`: logs mpv's timing counters every 10 s.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use libmpv2::Mpv;

use crate::config::{DisplayRate, Settings};

/// What `EMBYCLIENTPLUS_DISPLAY_FPS` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DisplayFps {
    /// Not set: the Preferences setting decides.
    #[default]
    Setting,
    /// Set no `display-fps-override`.
    Off,
    Value(f64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Switches {
    pub video_sync: Option<String>,
    pub display_fps: DisplayFps,
    pub report_swap: bool,
}

impl Default for Switches {
    fn default() -> Self {
        Self {
            video_sync: None,
            display_fps: DisplayFps::Setting,
            report_swap: true,
        }
    }
}

impl Switches {
    pub fn parse(var: impl Fn(&str) -> Option<String>) -> Self {
        let mut out = Self {
            video_sync: var("EMBYCLIENTPLUS_VIDEO_SYNC")
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty()),
            ..Self::default()
        };
        if let Some(v) = var("EMBYCLIENTPLUS_DISPLAY_FPS") {
            let v = v.trim();
            out.display_fps = if v.eq_ignore_ascii_case("off") {
                DisplayFps::Off
            } else {
                match v.parse::<f64>() {
                    Ok(n) if n.is_finite() && n > 0.0 => DisplayFps::Value(n),
                    _ => {
                        tracing::warn!("ignoring EMBYCLIENTPLUS_DISPLAY_FPS={v:?}");
                        DisplayFps::Setting
                    }
                }
            };
        }
        if let Some(v) = var("EMBYCLIENTPLUS_REPORT_SWAP") {
            out.report_swap = v.trim() != "0";
        }
        out
    }

    /// The `display-fps-override` for the setting and a monitor rate in
    /// millihertz, or `None` for no override. The env switch wins.
    pub fn display_fps_for(&self, rate: DisplayRate, millihertz: i32) -> Option<f64> {
        match (self.display_fps, rate) {
            (DisplayFps::Off, _) | (DisplayFps::Setting, DisplayRate::Off) => None,
            (DisplayFps::Value(n), _) | (DisplayFps::Setting, DisplayRate::Fixed(n)) => Some(n),
            (DisplayFps::Setting, DisplayRate::Automatic) => {
                (millihertz > 0).then(|| f64::from(millihertz) / 1000.0)
            }
        }
    }
}

/// The switches, read from the environment on first use.
pub fn switches() -> &'static Switches {
    static SWITCHES: OnceLock<Switches> = OnceLock::new();
    SWITCHES.get_or_init(|| {
        let s = Switches::parse(|name| std::env::var(name).ok());
        if s != Switches::default() {
            tracing::info!(
                "test switches: video-sync={:?} display-fps={:?} report-swap={}",
                s.video_sync,
                s.display_fps,
                s.report_swap
            );
        }
        s
    })
}

static MONITOR_MHZ: AtomicU32 = AtomicU32::new(0);
static PAINTED: AtomicU64 = AtomicU64::new(0);
static SWAPS: AtomicU64 = AtomicU64::new(0);
/// Every frame the window's frame clock painted, video or not: far above
/// `PAINTED` means GTK is redrawing something else (an animation without
/// vsync pacing can take the whole GPU/CPU).
static WINDOW_FRAMES: AtomicU64 = AtomicU64::new(0);
static LOADED: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);

pub fn note_monitor(millihertz: i32) {
    MONITOR_MHZ.store(u32::try_from(millihertz).unwrap_or(0), Relaxed);
}

/// Tells mpv the display rate the setting (and env switch) ask for; the
/// monitor's rate is the one last noted. No override clears a previous one.
pub fn apply_display_rate(player: super::Player) {
    let chosen = Settings::load().ok().and_then(|s| s.video.display_rate);
    let rate = DisplayRate::resolve(chosen, crate::gamescope::detected());
    let mhz = i32::try_from(MONITOR_MHZ.load(Relaxed)).unwrap_or(0);
    let fps = switches().display_fps_for(rate, mhz).unwrap_or(0.0);
    if let Err(e) = player.set_display_fps(fps) {
        tracing::warn!("{e:#}");
    }
}

/// mpv rendered a frame into the window.
pub fn note_paint() {
    PAINTED.fetch_add(1, Relaxed);
}

/// The window's frame clock finished a frame.
pub fn note_window_frame() {
    WINDOW_FRAMES.fetch_add(1, Relaxed);
}

/// `report_swap` was called.
pub fn note_swap() {
    SWAPS.fetch_add(1, Relaxed);
}

pub fn set_loaded(loaded: bool) {
    LOADED.store(loaded, Relaxed);
}

pub fn set_paused(paused: bool) {
    PAUSED.store(paused, Relaxed);
}

fn prop<T: libmpv2::GetData>(mpv: &Mpv, name: &str) -> Option<T> {
    mpv.get_property::<T>(name).ok()
}

fn num(v: Option<f64>) -> String {
    v.map_or_else(|| "n/a".into(), |v| format!("{v:.3}"))
}

fn text(v: Option<String>) -> String {
    v.filter(|v| !v.is_empty()).unwrap_or_else(|| "n/a".into())
}

/// One line with the timing setup, at playback start and when SVP changes.
pub fn log_config(mpv: &Mpv, reason: &str) {
    let monitor = match MONITOR_MHZ.load(Relaxed) {
        0 => "n/a".to_owned(),
        mhz => format!("{:.3}Hz", f64::from(mhz) / 1000.0),
    };
    let override_fps = num(prop::<f64>(mpv, "display-fps-override"));
    let svp_on = super::SVP_ON.load(Relaxed);
    let attached = prop::<String>(mpv, "vf").is_some_and(|vf| vf.contains("svp"));
    tracing::info!(
        target: "embyclientplus::player::stats",
        "timing setup ({reason}): gamescope={} monitor-refresh={monitor} display-fps-override={override_fps} \
         video-sync={} hwdec-current={} vo={} svp(on={svp_on}, attached={attached})",
        crate::gamescope::detected(),
        text(prop(mpv, "video-sync")),
        text(prop(mpv, "hwdec-current")),
        text(prop(mpv, "current-vo")),
    );
}

#[derive(Default, Clone, Copy)]
struct Counters {
    frame_drop: i64,
    decoder_drop: i64,
    mistimed: i64,
    vo_delayed: i64,
    painted: u64,
    swaps: u64,
    frames: u64,
}

const INTERVAL: Duration = Duration::from_secs(10);

/// Logs mpv's timing properties every 10 s while a file plays unpaused
/// (`EMBYCLIENTPLUS_STATS=1` only).
/// Reading properties is cheap and runs off the GTK thread.
pub fn spawn_stats_thread(mpv: &'static Mpv) {
    // Each property read is answered on mpv's playback thread, between
    // frames, so these lines are opt-in.
    if !std::env::var("EMBYCLIENTPLUS_STATS").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    tracing::info!("timing stats on (EMBYCLIENTPLUS_STATS=1)");
    std::thread::spawn(move || {
        let mut prev: Option<Counters> = None;
        loop {
            std::thread::sleep(INTERVAL);
            if !LOADED.load(Relaxed) || PAUSED.load(Relaxed) {
                prev = None;
                continue;
            }
            let now = Counters {
                frame_drop: prop(mpv, "frame-drop-count").unwrap_or(0),
                decoder_drop: prop(mpv, "decoder-frame-drop-count").unwrap_or(0),
                mistimed: prop(mpv, "mistimed-frame-count").unwrap_or(0),
                vo_delayed: prop(mpv, "vo-delayed-frame-count").unwrap_or(0),
                painted: PAINTED.load(Relaxed),
                swaps: SWAPS.load(Relaxed),
                frames: WINDOW_FRAMES.load(Relaxed),
            };
            // After a pause the counters were not sampled: this line then
            // covers everything since the last baseline (or the file start).
            let before = prev.unwrap_or(Counters {
                painted: now.painted,
                swaps: now.swaps,
                frames: now.frames,
                ..Counters::default()
            });
            prev = Some(now);
            let vf: String = text(prop(mpv, "vf")).chars().take(80).collect();
            tracing::info!(
                target: "embyclientplus::player::stats",
                "container-fps={} vf-fps={} display-fps={} est-display-fps={} video-sync={} \
                 drops(frame={:+}, decoder={:+}, mistimed={:+}, vo-delayed={:+}) \
                 vsync-jitter={} avsync={} pixfmt={} vf={vf:?} painted={} swaps={} frames={}",
                num(prop(mpv, "container-fps")),
                num(prop(mpv, "estimated-vf-fps")),
                num(prop(mpv, "display-fps")),
                num(prop(mpv, "estimated-display-fps")),
                text(prop(mpv, "video-sync")),
                now.frame_drop - before.frame_drop,
                now.decoder_drop - before.decoder_drop,
                now.mistimed - before.mistimed,
                now.vo_delayed - before.vo_delayed,
                num(prop(mpv, "vsync-jitter")),
                num(prop(mpv, "avsync")),
                text(prop(mpv, "video-params/pixelformat")),
                now.painted.saturating_sub(before.painted),
                now.swaps.saturating_sub(before.swaps),
                now.frames.saturating_sub(before.frames),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{DisplayFps, Switches};
    use crate::config::DisplayRate;

    fn parse(pairs: &[(&str, &str)]) -> Switches {
        Switches::parse(|name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        })
    }

    #[test]
    fn defaults_when_unset() {
        let s = parse(&[]);
        assert_eq!(s, Switches::default());
        assert_eq!(
            s.display_fps_for(DisplayRate::Automatic, 59_940),
            Some(59.94)
        );
        assert_eq!(s.display_fps_for(DisplayRate::Automatic, 0), None);
    }

    #[test]
    fn setting_applies_without_env() {
        let s = parse(&[]);
        assert_eq!(s.display_fps_for(DisplayRate::Off, 60_000), None);
        assert_eq!(
            s.display_fps_for(DisplayRate::Fixed(72.0), 60_000),
            Some(72.0)
        );
    }

    #[test]
    fn video_sync_override() {
        assert_eq!(
            parse(&[("EMBYCLIENTPLUS_VIDEO_SYNC", " audio ")]).video_sync,
            Some("audio".into())
        );
        assert_eq!(
            parse(&[("EMBYCLIENTPLUS_VIDEO_SYNC", " ")]).video_sync,
            None
        );
    }

    #[test]
    fn display_fps_env_wins_over_setting() {
        let s = parse(&[("EMBYCLIENTPLUS_DISPLAY_FPS", "48")]);
        assert_eq!(s.display_fps, DisplayFps::Value(48.0));
        for rate in [
            DisplayRate::Automatic,
            DisplayRate::Off,
            DisplayRate::Fixed(90.0),
        ] {
            assert_eq!(s.display_fps_for(rate, 60_000), Some(48.0));
        }
        let s = parse(&[("EMBYCLIENTPLUS_DISPLAY_FPS", "OFF")]);
        assert_eq!(s.display_fps_for(DisplayRate::Automatic, 60_000), None);
        assert_eq!(s.display_fps_for(DisplayRate::Fixed(90.0), 60_000), None);
        for bad in ["abc", "0", "-5", "nan"] {
            let s = parse(&[("EMBYCLIENTPLUS_DISPLAY_FPS", bad)]);
            assert_eq!(s.display_fps, DisplayFps::Setting, "{bad}");
        }
    }

    #[test]
    fn report_swap_only_off_for_zero() {
        assert!(!parse(&[("EMBYCLIENTPLUS_REPORT_SWAP", "0")]).report_swap);
        assert!(parse(&[("EMBYCLIENTPLUS_REPORT_SWAP", "1")]).report_swap);
        assert!(parse(&[]).report_swap);
    }
}
