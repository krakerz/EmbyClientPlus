//! Playback timing diagnostics and env test switches.
//!
//! Env switches (read once at startup, logged when set; for diagnosing
//! timing problems such as dropped frames under gamescope):
//! - `EMBYCLIENTPLUS_VIDEO_SYNC=<mode>`: replaces the mpv `video-sync` the
//!   app would choose (e.g. `audio`, `display-resample`).
//! - `EMBYCLIENTPLUS_DISPLAY_FPS=<number>|off`: a number replaces the
//!   monitor's refresh rate as `display-fps-override`; `off` sets none.
//!   Either is final: the rate auto-check below stays off.
//! - `EMBYCLIENTPLUS_REPORT_SWAP=0`: stops telling mpv when frames went out
//!   (which also turns the rate auto-check off, mpv can't measure then).
//!
//! Rate auto-check: the monitor's reported refresh rate isn't always the
//! rate frames are shown at (a Steam Deck in Game Mode reported 119.9 Hz
//! while gamescope presented ~60). Playback starts with the monitor's rate
//! as `display-fps-override`; once a second a thread samples mpv's
//! `estimated-display-fps` (measured from the swap timing) and
//! [`decide_rate`] follows it when it is stable and far from the override.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use libmpv2::Mpv;

/// What `EMBYCLIENTPLUS_DISPLAY_FPS` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DisplayFps {
    /// Use the monitor's refresh rate.
    #[default]
    Monitor,
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
            display_fps: DisplayFps::Monitor,
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
                        DisplayFps::Monitor
                    }
                }
            };
        }
        if let Some(v) = var("EMBYCLIENTPLUS_REPORT_SWAP") {
            out.report_swap = v.trim() != "0";
        }
        out
    }

    /// The `display-fps-override` to start with for a monitor rate in
    /// millihertz, or `None` for no override.
    pub fn display_fps_for(&self, millihertz: i32) -> Option<f64> {
        match self.display_fps {
            DisplayFps::Off => None,
            DisplayFps::Value(n) => Some(n),
            DisplayFps::Monitor => (millihertz > 0).then(|| f64::from(millihertz) / 1000.0),
        }
    }

    /// Whether the override follows mpv's measured display rate.
    pub fn auto_check(&self) -> bool {
        self.display_fps == DisplayFps::Monitor && self.report_swap
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
/// Bumped on every monitor change, so the rate watcher starts over.
static MONITOR_GEN: AtomicU64 = AtomicU64::new(0);
/// Milliseconds since `epoch()` until which rate samples are ignored.
static SETTLE_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
static PAINTED: AtomicU64 = AtomicU64::new(0);
static SWAPS: AtomicU64 = AtomicU64::new(0);
static LOADED: AtomicBool = AtomicBool::new(false);
static PAUSED: AtomicBool = AtomicBool::new(false);

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    u64::try_from(epoch().elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub fn note_monitor(millihertz: i32) {
    MONITOR_MHZ.store(u32::try_from(millihertz).unwrap_or(0), Relaxed);
    MONITOR_GEN.fetch_add(1, Relaxed);
    note_disturbance();
}

/// A file loaded, playback resumed or seeked, or the monitor changed: the
/// display rate measured over the next seconds isn't steady yet.
pub fn note_disturbance() {
    let settle = u64::try_from(SETTLE.as_millis()).unwrap_or(0);
    SETTLE_UNTIL_MS.store(now_ms() + settle, Relaxed);
}

/// mpv rendered a frame into the window.
pub fn note_paint() {
    PAINTED.fetch_add(1, Relaxed);
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
    let auto_check = if switches().auto_check() { "on" } else { "off" };
    tracing::info!(
        target: "embyclientplus::player::stats",
        "timing setup ({reason}): gamescope={} monitor-refresh={monitor} display-fps-override={override_fps} \
         auto-check={auto_check} video-sync={} hwdec-current={} vo={} svp(on={svp_on}, attached={attached})",
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
}

const INTERVAL: Duration = Duration::from_secs(10);

/// Logs mpv's timing properties every 10 s while a file plays unpaused.
/// Reading properties is cheap and runs off the GTK thread.
pub fn spawn_stats_thread(mpv: &'static Mpv) {
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
            };
            // After a pause the counters were not sampled: this line then
            // covers everything since the last baseline (or the file start).
            let before = prev.unwrap_or(Counters {
                painted: now.painted,
                swaps: now.swaps,
                ..Counters::default()
            });
            prev = Some(now);
            let vf: String = text(prop(mpv, "vf")).chars().take(80).collect();
            tracing::info!(
                target: "embyclientplus::player::stats",
                "container-fps={} vf-fps={} display-fps={} est-display-fps={} video-sync={} \
                 drops(frame={:+}, decoder={:+}, mistimed={:+}, vo-delayed={:+}) \
                 vsync-jitter={} avsync={} pixfmt={} vf={vf:?} painted={} swaps={}",
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
            );
        }
    });
}

/// Samples that have to agree before the override follows them.
const WINDOW: usize = 5;
/// Samples this far (relative) from the override count as "far".
const FAR: f64 = 0.10;
/// Samples within this spread (relative to their median) agree.
const AGREE: f64 = 0.03;
/// At most one change of the override per this long.
const MIN_SWITCH_GAP: Duration = Duration::from_secs(10);
/// Rate samples are skipped this long after a load, unpause, seek or
/// monitor change.
const SETTLE: Duration = Duration::from_secs(3);
const SAMPLE_EVERY: Duration = Duration::from_secs(1);

/// What to do with `display-fps-override` after a new rate sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RateDecision {
    Keep,
    /// Use this rate.
    Switch(f64),
    /// The measured rate is far off and unstable: use no override.
    NoOverride,
}

/// Decides from the recent `samples` of mpv's `estimated-display-fps` (valid
/// ones, consecutive, newest last), the current override and the time since
/// the last change. The last [`WINDOW`] samples all being far from the
/// override and agreeing means frames are shown at their median rate; far
/// but disagreeing means the rate can't be trusted either way.
pub fn decide_rate(
    current: Option<f64>,
    samples: &[f64],
    since_switch: Option<Duration>,
) -> RateDecision {
    if samples.len() < WINDOW || since_switch.is_some_and(|d| d < MIN_SWITCH_GAP) {
        return RateDecision::Keep;
    }
    let recent = &samples[samples.len() - WINDOW..];
    if recent.iter().any(|s| !s.is_finite() || *s <= 0.0) {
        return RateDecision::Keep;
    }
    let far = |s: f64| current.is_none_or(|c| (s - c).abs() > c * FAR);
    if !recent.iter().copied().all(far) {
        return RateDecision::Keep;
    }
    let mut sorted = recent.to_vec();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[WINDOW / 2];
    let spread = (sorted[WINDOW - 1] - sorted[0]) / median;
    if spread <= AGREE {
        RateDecision::Switch(median)
    } else if current.is_some() {
        RateDecision::NoOverride
    } else {
        RateDecision::Keep
    }
}

/// Follows mpv's measured display rate with `display-fps-override`, once a
/// second on its own thread (reading a property is cheap and never touches
/// GTK). Does nothing when the switches make the override final.
pub fn spawn_rate_watch(mpv: &'static Mpv) {
    let s = switches();
    if !s.auto_check() {
        let why = if s.display_fps == DisplayFps::Monitor {
            "EMBYCLIENTPLUS_REPORT_SWAP=0, mpv can't measure the rate"
        } else {
            "EMBYCLIENTPLUS_DISPLAY_FPS is set"
        };
        tracing::info!("display rate auto-check off: {why}");
        return;
    }
    std::thread::spawn(move || {
        let mut generation = u64::MAX;
        let mut current: Option<f64> = None;
        let mut samples: Vec<f64> = Vec::with_capacity(WINDOW + 1);
        let mut last_switch: Option<Instant> = None;
        loop {
            std::thread::sleep(SAMPLE_EVERY);
            let seen = MONITOR_GEN.load(Relaxed);
            if seen != generation {
                // New monitor (or the first one): start over from its rate.
                generation = seen;
                current = s.display_fps_for(i32::try_from(MONITOR_MHZ.load(Relaxed)).unwrap_or(0));
                samples.clear();
                last_switch = None;
            }
            if !LOADED.load(Relaxed)
                || PAUSED.load(Relaxed)
                || now_ms() < SETTLE_UNTIL_MS.load(Relaxed)
            {
                samples.clear();
                continue;
            }
            let Some(rate) = prop::<f64>(mpv, "estimated-display-fps")
                .filter(|r| r.is_finite() && (1.0..=1000.0).contains(r))
            else {
                samples.clear();
                continue;
            };
            samples.push(rate);
            if samples.len() > WINDOW {
                samples.remove(0);
            }
            let decision = decide_rate(current, &samples, last_switch.map(|t| t.elapsed()));
            if decision == RateDecision::Keep || MONITOR_GEN.load(Relaxed) != generation {
                continue;
            }
            let monitor = f64::from(MONITOR_MHZ.load(Relaxed)) / 1000.0;
            let (value, line) = match decision {
                RateDecision::Switch(to) => (
                    to,
                    format!(
                        "display rate: monitor reports {monitor:.3} Hz, frames are shown at ~{to:.3} Hz \
                         (override was {}) -> using {to:.3}",
                        current.map_or_else(|| "off".to_owned(), |c| format!("{c:.3}")),
                    ),
                ),
                _ => (
                    0.0,
                    format!(
                        "display rate: monitor reports {monitor:.3} Hz, measured rate unstable ({}) \
                         -> no override",
                        samples
                            .iter()
                            .map(|r| format!("{r:.1}"))
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                ),
            };
            match mpv.set_property("display-fps-override", value) {
                Ok(()) => {
                    tracing::info!("{line}");
                    current = (value > 0.0).then_some(value);
                    last_switch = Some(Instant::now());
                    samples.clear();
                }
                Err(e) => tracing::warn!("failed to set display-fps-override: {e:?}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{DisplayFps, RateDecision, Switches, decide_rate};

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
        assert_eq!(s.display_fps_for(59_940), Some(59.94));
        assert_eq!(s.display_fps_for(0), None);
        assert!(s.auto_check());
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
    fn display_fps_values() {
        let s = parse(&[("EMBYCLIENTPLUS_DISPLAY_FPS", "48")]);
        assert_eq!(s.display_fps, DisplayFps::Value(48.0));
        assert_eq!(s.display_fps_for(60_000), Some(48.0));
        assert!(!s.auto_check());
        let s = parse(&[("EMBYCLIENTPLUS_DISPLAY_FPS", "OFF")]);
        assert_eq!(s.display_fps_for(60_000), None);
        assert!(!s.auto_check());
        for bad in ["abc", "0", "-5", "nan"] {
            let s = parse(&[("EMBYCLIENTPLUS_DISPLAY_FPS", bad)]);
            assert_eq!(s.display_fps, DisplayFps::Monitor, "{bad}");
        }
    }

    #[test]
    fn report_swap_only_off_for_zero() {
        assert!(!parse(&[("EMBYCLIENTPLUS_REPORT_SWAP", "0")]).report_swap);
        assert!(parse(&[("EMBYCLIENTPLUS_REPORT_SWAP", "1")]).report_swap);
        assert!(parse(&[]).report_swap);
        assert!(!parse(&[("EMBYCLIENTPLUS_REPORT_SWAP", "0")]).auto_check());
    }

    const LONG: Option<Duration> = Some(Duration::from_secs(60));

    #[test]
    fn deck_follows_measured_rate() {
        let samples = [59.9, 59.8, 60.0, 59.9, 59.95];
        assert_eq!(
            decide_rate(Some(119.903), &samples, None),
            RateDecision::Switch(59.9)
        );
    }

    #[test]
    fn matching_rate_is_kept() {
        let samples = [164.9, 164.8, 165.0, 164.9, 164.85];
        assert_eq!(
            decide_rate(Some(164.835), &samples, None),
            RateDecision::Keep
        );
    }

    #[test]
    fn unstable_far_samples_drop_the_override() {
        let samples = [40.0, 59.9, 52.0, 60.0, 45.0];
        assert_eq!(
            decide_rate(Some(119.903), &samples, None),
            RateDecision::NoOverride
        );
        // Already none: nothing to change.
        assert_eq!(decide_rate(None, &samples, None), RateDecision::Keep);
        // Unstable, but some samples near the override: keep.
        let mixed = [119.0, 59.9, 118.0, 60.0, 120.0];
        assert_eq!(decide_rate(Some(119.903), &mixed, None), RateDecision::Keep);
    }

    #[test]
    fn too_few_samples_keep() {
        assert_eq!(
            decide_rate(Some(119.903), &[59.9, 59.9, 59.9, 59.9], None),
            RateDecision::Keep
        );
        assert_eq!(decide_rate(Some(119.903), &[], None), RateDecision::Keep);
    }

    #[test]
    fn only_the_latest_window_counts() {
        let samples = [164.9, 164.9, 59.9, 59.8, 60.0, 59.9, 59.95];
        assert_eq!(
            decide_rate(Some(164.835), &samples, None),
            RateDecision::Switch(59.9)
        );
        // One stray sample inside the window: far from none, so keep.
        let samples = [59.9, 59.9, 165.0, 59.9, 59.9];
        assert_eq!(
            decide_rate(Some(164.835), &samples, None),
            RateDecision::Keep
        );
    }

    #[test]
    fn switches_at_most_every_ten_seconds() {
        let samples = [59.9, 59.8, 60.0, 59.9, 59.95];
        assert_eq!(
            decide_rate(Some(119.903), &samples, Some(Duration::from_secs(9))),
            RateDecision::Keep
        );
        assert_eq!(
            decide_rate(Some(119.903), &samples, Some(Duration::from_secs(10))),
            RateDecision::Switch(59.9)
        );
    }

    #[test]
    fn follows_back_to_native_rate() {
        // Switched to 59.9, then a frame limiter is lifted and ~119.9 shows up.
        let samples = [119.9, 119.8, 119.9, 120.0, 119.9];
        assert_eq!(
            decide_rate(Some(59.9), &samples, LONG),
            RateDecision::Switch(119.9)
        );
        // After "no override", stable samples are adopted.
        assert_eq!(
            decide_rate(None, &[59.9, 59.9, 59.8, 60.0, 59.9], LONG),
            RateDecision::Switch(59.9)
        );
    }
}
