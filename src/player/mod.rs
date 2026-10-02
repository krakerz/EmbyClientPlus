pub mod video_area;

use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use libmpv2::events::{Event, PropertyData};
use libmpv2::{EndFileReason, Format, Mpv};

/// Fixed path SVP Manager looks for (see `~/SVP4/mpv/mpv.conf`); it attaches
/// here and injects its vapoursynth filter over JSON IPC.
const SVP_IPC_SOCKET: &str = "/tmp/mpvsocket";

/// Label SVP Manager gives the vapoursynth filter it adds.
const SVP_FILTER_LABEL: &str = "@svp";

/// Display size before and after the filter chain; they differ once SVP's
/// black-bar lighting pads the frame.
const GEOMETRY_PROPERTIES: [&str; 4] = [
    "video-params/dw",
    "video-params/dh",
    "video-out-params/dw",
    "video-out-params/dh",
];

/// Observed properties beyond geometry, with the formats they're read in.
/// `time-pos` is deliberately absent: it changes every frame (120 fps under
/// SVP), so the UI polls it instead.
const STATE_PROPERTIES: [(&str, Format); 7] = [
    ("pause", Format::Flag),
    ("duration", Format::Double),
    ("track-list", Format::String),
    ("aid", Format::String),
    ("sid", Format::String),
    ("volume", Format::Double),
    ("mute", Format::Flag),
];

pub const END_FILE_REASON_EOF: EndFileReason = 0;
pub const END_FILE_REASON_ERROR: EndFileReason = 4;

/// Something the UI may need to react to, sent from mpv's event thread.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    /// The video's pre- or post-filter size changed.
    Geometry,
    Pause(bool),
    Duration(f64),
    /// Track list or the selected audio/subtitle track changed.
    Tracks,
    /// Volume or mute changed.
    Volume,
    FileLoaded,
    /// Playback ended: end of file, error, or our own stop/replace.
    Finished(EndFileReason),
}

static EVENT_HANDLER: OnceLock<Box<dyn Fn(PlayerEvent) + Send + Sync>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Subtitle,
    Video,
}

/// One entry of mpv's `track-list`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    pub id: i64,
    pub kind: Option<TrackKind>,
    pub selected: bool,
    pub lang: Option<String>,
    pub title: Option<String>,
    pub codec: Option<String>,
    pub default: bool,
    pub forced: bool,
    pub external: bool,
    /// Stream index inside the container (matches Emby's `MediaStream.Index`
    /// for embedded streams).
    pub ff_index: Option<i64>,
    pub external_filename: Option<String>,
}

/// Handle to the app's single libmpv instance.
#[derive(Clone, Copy)]
pub struct Player {
    mpv: &'static Mpv,
}

impl Player {
    /// Must run after GTK init: GTK applies the user's locale, and libmpv
    /// refuses to initialize unless LC_NUMERIC is "C".
    pub fn new() -> Result<Self> {
        // SAFETY: called on the GTK main thread before any mpv threads exist.
        unsafe { libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr()) };
        let mpv = Mpv::with_initializer(|init| {
            // Render-API output only; without this mpv may open its own window.
            init.set_property("vo", "libmpv")?;
            // mpv's messages go through `tracing` (and the log file) instead.
            init.set_property("terminal", "no")?;
            // Mirrors SVP's own [svp] mpv.conf profile: vapoursynth needs
            // decoded frames in system memory, and frame-dropping hr-seeks
            // desync audio once the filter is active.
            init.set_property("hwdec", "auto-copy")?;
            init.set_property("hr-seek-framedrop", "no")?;
            // We pick tracks ourselves once the file is loaded.
            init.set_property("sub-auto", "no")?;
            Ok(())
        })
        .map_err(|e| anyhow!("failed to initialize libmpv: {e:?}"))?;

        // One instance for the app's whole lifetime: render contexts borrow
        // it, and the event thread needs it too.
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));
        // SAFETY: valid handle for the process lifetime; static C string.
        let status =
            unsafe { libmpv2_sys::mpv_request_log_messages(mpv.ctx.as_ptr(), c"info".as_ptr()) };
        if status < 0 {
            tracing::warn!("mpv log messages unavailable (error {status})");
        }
        let observed = GEOMETRY_PROPERTIES
            .iter()
            .map(|&name| (name, Format::Int64))
            .chain(STATE_PROPERTIES);
        for (id, (name, format)) in (1..).zip(observed) {
            mpv.observe_property(name, format, id)
                .map_err(|e| anyhow!("failed to observe {name}: {e:?}"))?;
        }
        spawn_event_thread(mpv);
        Ok(Self { mpv })
    }

    pub(crate) fn mpv(self) -> &'static Mpv {
        self.mpv
    }

    /// Registers the handler run (on mpv's event thread) for every
    /// [`PlayerEvent`]. Only the first registration sticks.
    pub fn on_event(self, handler: impl Fn(PlayerEvent) + Send + Sync + 'static) {
        let _ = EVENT_HANDLER.set(Box::new(handler));
    }

    /// Zooms so the original picture fills the window even when SVP's
    /// black-bar lighting has padded the frame to the monitor's aspect ratio
    /// (SVP assumes fullscreen); the glow then only shows in real spare space.
    pub fn fit_to_window(self, window_width: i32, window_height: i32) -> Result<()> {
        let Some([src_w, src_h, out_w, out_h]) = self.geometry() else {
            return Ok(()); // no video yet
        };
        let zoom = padding_zoom(
            (src_w as f64, src_h as f64),
            (out_w as f64, out_h as f64),
            (window_width as f64, window_height as f64),
        );
        self.mpv
            .set_property("video-zoom", zoom)
            .map_err(|e| anyhow!("failed to set video-zoom: {e:?}"))
    }

    fn geometry(self) -> Option<[i64; 4]> {
        let mut values = [0; 4];
        for (value, name) in values.iter_mut().zip(GEOMETRY_PROPERTIES) {
            *value = self.mpv.get_property::<i64>(name).ok()?;
        }
        values.iter().all(|&v| v > 0).then_some(values)
    }

    pub fn load(self, target: &str) -> Result<()> {
        self.command("loadfile", &[target])
    }

    /// Plays `target` from `start_seconds`, with extra subtitle files
    /// loaded alongside (they show up in `track-list` by URL).
    pub fn load_at(self, target: &str, start_seconds: f64, subtitle_files: &[&str]) -> Result<()> {
        // mpv ≥ 0.38 takes the playlist index before the per-file options.
        self.command(
            "loadfile",
            &[
                target,
                "replace",
                "-1",
                &file_options(start_seconds, subtitle_files),
            ],
        )
    }

    pub fn stop(self) -> Result<()> {
        self.command("stop", &[])
    }

    /// Current playback position, once a file is playing.
    pub fn position(self) -> Option<f64> {
        self.mpv.get_property::<f64>("time-pos").ok()
    }

    pub fn duration(self) -> Option<f64> {
        self.mpv
            .get_property::<f64>("duration")
            .ok()
            .filter(|d| *d > 0.0)
    }

    pub fn is_paused(self) -> bool {
        self.mpv.get_property::<bool>("pause").unwrap_or(false)
    }

    pub fn toggle_pause(self) -> Result<()> {
        self.command("cycle", &["pause"])
    }

    pub fn seek_relative(self, seconds: i32) -> Result<()> {
        self.command("seek", &[&seconds.to_string(), "relative"])
    }

    pub fn seek_absolute(self, seconds: f64) -> Result<()> {
        self.command("seek", &[&format!("{seconds:.3}"), "absolute"])
    }

    /// 0–100 (mpv allows up to `volume-max`, 130 by default).
    pub fn volume(self) -> f64 {
        self.mpv.get_property::<f64>("volume").unwrap_or(100.0)
    }

    pub fn set_volume(self, volume: f64) -> Result<()> {
        self.set("volume", volume.clamp(0.0, 130.0))
    }

    pub fn is_muted(self) -> bool {
        self.mpv.get_property::<bool>("mute").unwrap_or(false)
    }

    pub fn toggle_mute(self) -> Result<()> {
        self.command("cycle", &["mute"])
    }

    pub fn tracks(self) -> Vec<Track> {
        let count = self
            .mpv
            .get_property::<i64>("track-list/count")
            .unwrap_or(0);
        (0..count).filter_map(|n| self.track(n)).collect()
    }

    fn track(self, n: i64) -> Option<Track> {
        let string = |key: &str| {
            self.mpv
                .get_property::<String>(&format!("track-list/{n}/{key}"))
                .ok()
                .filter(|v| !v.is_empty())
        };
        let flag = |key: &str| {
            self.mpv
                .get_property::<bool>(&format!("track-list/{n}/{key}"))
                .unwrap_or(false)
        };
        let int = |key: &str| {
            self.mpv
                .get_property::<i64>(&format!("track-list/{n}/{key}"))
                .ok()
        };
        Some(Track {
            id: int("id")?,
            kind: string("type").as_deref().and_then(track_kind),
            selected: flag("selected"),
            lang: string("lang"),
            title: string("title"),
            codec: string("codec"),
            default: flag("default"),
            forced: flag("forced"),
            external: flag("external"),
            ff_index: int("ff-index"),
            external_filename: string("external-filename"),
        })
    }

    pub fn set_audio(self, id: i64) -> Result<()> {
        self.set("aid", id)
    }

    /// `None` turns subtitles off.
    pub fn set_subtitle(self, id: Option<i64>) -> Result<()> {
        match id {
            Some(id) => self.set("sid", id),
            None => self.set("sid", "no"),
        }
    }

    /// Exposes (or closes) the socket SVP Manager attaches to. Turning it
    /// off also drops the filter SVP already added; turning it on lets SVP
    /// Manager find us again.
    pub fn set_svp(self, enabled: bool) -> Result<()> {
        if enabled {
            self.set("input-ipc-server", SVP_IPC_SOCKET)
        } else {
            self.set("input-ipc-server", "")?;
            // Not an error when SVP never attached.
            let _ = self.command("vf", &["remove", SVP_FILTER_LABEL]);
            Ok(())
        }
    }

    /// Whether SVP Manager has added its filter to this playback.
    pub fn svp_attached(self) -> bool {
        self.mpv
            .get_property::<String>("vf")
            .is_ok_and(|vf| vf.contains("svp"))
    }

    pub fn quit(self) -> Result<()> {
        self.command("quit", &[])
    }

    fn set<T: libmpv2::SetData>(self, name: &str, value: T) -> Result<()> {
        self.mpv
            .set_property(name, value)
            .map_err(|e| anyhow!("failed to set {name}: {e:?}"))
    }

    fn command(self, name: &str, args: &[&str]) -> Result<()> {
        self.mpv
            .command(name, args)
            .map_err(|e| anyhow!("mpv command {name} failed: {e:?}"))
    }
}

/// Per-file `loadfile` options. Values use mpv's `%len%` quoting, since
/// URLs can contain the `,` and `=` that separate options.
fn file_options(start_seconds: f64, subtitle_files: &[&str]) -> String {
    let mut options = vec![format!("start={start_seconds:.3}")];
    for file in subtitle_files {
        options.push(format!("sub-files-append=%{}%{file}", file.len()));
    }
    options.join(",")
}

fn track_kind(kind: &str) -> Option<TrackKind> {
    match kind {
        "audio" => Some(TrackKind::Audio),
        "sub" => Some(TrackKind::Subtitle),
        "video" => Some(TrackKind::Video),
        _ => None,
    }
}

fn spawn_event_thread(mpv: &'static Mpv) {
    std::thread::spawn(move || {
        loop {
            let event = match mpv.wait_event(-1.0) {
                Some(Ok(Event::Shutdown)) => break,
                Some(Ok(Event::FileLoaded)) => Some(PlayerEvent::FileLoaded),
                Some(Ok(Event::LogMessage {
                    prefix,
                    level,
                    text,
                    ..
                })) => {
                    log_mpv(prefix, level, text);
                    None
                }
                Some(Ok(Event::EndFile(reason))) => {
                    tracing::info!("playback ended (reason {reason})");
                    Some(PlayerEvent::Finished(reason))
                }
                Some(Ok(Event::PropertyChange { name, change, .. })) => {
                    property_event(name, &change)
                }
                Some(Err(e)) => {
                    tracing::warn!("mpv event error: {e:?}");
                    None
                }
                _ => None,
            };
            if let (Some(event), Some(handler)) = (event, EVENT_HANDLER.get()) {
                handler(event);
            }
        }
    });
}

fn log_mpv(prefix: &str, level: &str, text: &str) {
    let text = text.trim_end();
    match level {
        "fatal" | "error" => tracing::error!(target: "mpv", "[{prefix}] {text}"),
        "warn" => tracing::warn!(target: "mpv", "[{prefix}] {text}"),
        "info" => tracing::info!(target: "mpv", "[{prefix}] {text}"),
        _ => tracing::debug!(target: "mpv", "[{prefix}] {text}"),
    }
}

fn property_event(name: &str, change: &PropertyData) -> Option<PlayerEvent> {
    match (name, change) {
        ("pause", PropertyData::Flag(paused)) => Some(PlayerEvent::Pause(*paused)),
        ("duration", PropertyData::Double(duration)) => Some(PlayerEvent::Duration(*duration)),
        ("track-list" | "aid" | "sid", _) => Some(PlayerEvent::Tracks),
        ("volume" | "mute", _) => Some(PlayerEvent::Volume),
        (name, _) if GEOMETRY_PROPERTIES.contains(&name) => Some(PlayerEvent::Geometry),
        _ => None,
    }
}

/// `video-zoom` (log2 scale, mpv's convention) that fits the original
/// `source`-aspect picture inside the `output` frame to `window`, instead of
/// fitting the whole output frame. 0 when no padding was added.
fn padding_zoom(source: (f64, f64), output: (f64, f64), window: (f64, f64)) -> f64 {
    let (src_w, src_h) = source;
    let (out_w, out_h) = output;
    let (win_w, win_h) = window;
    let src_aspect = src_w / src_h;
    // The original picture's extent within the (possibly padded) output frame.
    let (core_w, core_h) = if out_w / out_h > src_aspect {
        (out_h * src_aspect, out_h)
    } else {
        (out_w, out_w / src_aspect)
    };
    let fit_output = (win_w / out_w).min(win_h / out_h);
    let fit_core = (win_w / core_w).min(win_h / core_h);
    (fit_core / fit_output).log2().max(0.0)
}

#[cfg(test)]
mod tests {
    use super::{PlayerEvent, file_options, padding_zoom, property_event};
    use libmpv2::events::PropertyData;

    #[test]
    fn file_options_quote_subtitle_urls() {
        assert_eq!(
            file_options(12.5, &["http://s/a.srt?x=1,2"]),
            "start=12.500,sub-files-append=%20%http://s/a.srt?x=1,2"
        );
        assert_eq!(file_options(0.0, &[]), "start=0.000");
    }

    #[test]
    fn property_changes_map_to_events() {
        assert_eq!(
            property_event("pause", &PropertyData::Flag(true)),
            Some(PlayerEvent::Pause(true))
        );
        assert_eq!(
            property_event("sid", &PropertyData::Str("2")),
            Some(PlayerEvent::Tracks)
        );
        assert_eq!(
            property_event("video-out-params/dw", &PropertyData::Int64(2576)),
            Some(PlayerEvent::Geometry)
        );
        assert_eq!(property_event("time-pos", &PropertyData::Double(1.0)), None);
    }

    fn scale(zoom: f64) -> f64 {
        zoom.exp2()
    }

    #[test]
    fn no_padding_means_no_zoom() {
        assert_eq!(
            padding_zoom((1920., 1080.), (1920., 1080.), (1280., 720.)),
            0.0
        );
        // Real scope film in a 16:9 window keeps its normal letterbox.
        assert_eq!(
            padding_zoom((1920., 804.), (1920., 804.), (1280., 720.)),
            0.0
        );
    }

    #[test]
    fn svp_side_padding_fills_window_height_again() {
        // 16:9 source padded to 2.389:1 by SVP, shown in a 16:9 window: the
        // original picture should fill the window exactly.
        let out_w = 1080. * 2.389;
        let zoom = padding_zoom((1920., 1080.), (out_w, 1080.), (1280., 720.));
        assert!((scale(zoom) - 2.389 / (16. / 9.)).abs() < 1e-9);
    }

    #[test]
    fn ultrawide_window_shows_the_full_glow() {
        // Window already at the padded aspect: nothing to crop.
        let out_w = 1080. * 2.389;
        let zoom = padding_zoom((1920., 1080.), (out_w, 1080.), (2389., 1000.));
        assert!(zoom.abs() < 1e-9);
    }
}
