pub mod compositor;
pub mod user_config;
pub mod video_area;

use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use libmpv2::events::{Event, PropertyData};
use libmpv2::{EndFileReason, Format, Mpv};

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
const STATE_PROPERTIES: [(&str, Format); 8] = [
    ("pause", Format::Flag),
    ("duration", Format::Double),
    ("track-list", Format::String),
    ("aid", Format::String),
    ("sid", Format::String),
    ("volume", Format::Double),
    ("mute", Format::Flag),
    ("playlist-pos", Format::Int64),
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
    /// mpv moved to another playlist entry (music queue).
    PlaylistPos(i64),
    /// Playback ended: end of file, error, or our own stop/replace.
    Finished(EndFileReason),
}

/// Extra zoom (log2) the user chose, added to the automatic fit.
static USER_ZOOM: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How the picture is shaped in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Aspect {
    /// The whole picture, bars where needed.
    #[default]
    Fit,
    /// Fill the window, cropping the overflow.
    Fill,
    /// Fill the window, distorting the picture.
    Stretch,
    Force16x9,
    Force4x3,
    Force21x9,
}

impl Aspect {
    pub const ALL: [Aspect; 6] = [
        Aspect::Fit,
        Aspect::Fill,
        Aspect::Stretch,
        Aspect::Force16x9,
        Aspect::Force4x3,
        Aspect::Force21x9,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Aspect::Fit => "fit",
            Aspect::Fill => "fill",
            Aspect::Stretch => "stretch",
            Aspect::Force16x9 => "16:9",
            Aspect::Force4x3 => "4:3",
            Aspect::Force21x9 => "21:9",
        }
    }

    pub fn from_name(name: &str) -> Option<Aspect> {
        Aspect::ALL.into_iter().find(|a| a.name() == name)
    }

    pub fn label(self) -> &'static str {
        match self {
            Aspect::Fit => "Fit",
            Aspect::Fill => "Fill (crop)",
            Aspect::Stretch => "Stretch",
            Aspect::Force16x9 => "16:9",
            Aspect::Force4x3 => "4:3",
            Aspect::Force21x9 => "21:9",
        }
    }

    /// (panscan, keepaspect, video-aspect-override) for mpv.
    fn mpv_properties(self) -> (f64, bool, &'static str) {
        match self {
            Aspect::Fit => (0.0, true, "no"),
            Aspect::Fill => (1.0, true, "no"),
            Aspect::Stretch => (0.0, false, "no"),
            Aspect::Force16x9 => (0.0, true, "16:9"),
            Aspect::Force4x3 => (0.0, true, "4:3"),
            Aspect::Force21x9 => (0.0, true, "2.39:1"),
        }
    }
}

/// The bar fill style (see `compositor`), read by the render callback.
static BAR_FILL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Mirrors whether frame blending is on, since mpv keeps it across files.
static SMOOTH_MOTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// Hardware decoders, in order (copy-back: SVP needs frames in RAM).
const HWDEC: &str = "vaapi-copy,auto-copy";

/// Handle to the app's single libmpv instance.
#[derive(Clone, Copy)]
pub struct Player {
    mpv: &'static Mpv,
}

impl Player {
    /// Must run after GTK init: GTK applies the user's locale, and libmpv
    /// refuses to initialize unless LC_NUMERIC is "C".
    /// `config_dir`: an mpv config directory to load (the user's mpv.conf,
    /// filtered; see `user_config`), or `None` for mpv's defaults only.
    pub fn new(config_dir: Option<&std::path::Path>) -> Result<Self> {
        // SAFETY: called on the GTK main thread before any mpv threads exist.
        unsafe { libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr()) };
        let mpv = Mpv::with_initializer(|init| {
            if let Some(dir) = config_dir {
                init.set_property("config-dir", dir.to_string_lossy().as_ref())?;
                init.set_property("config", "yes")?;
            }
            // Render-API output only; without this mpv may open its own window.
            init.set_property("vo", "libmpv")?;
            // mpv's messages go through `tracing` (and the log file) instead.
            init.set_property("terminal", "no")?;
            // Mirrors SVP's own [svp] mpv.conf profile: vapoursynth needs
            // decoded frames in system memory, and frame-dropping hr-seeks
            // desync audio once the filter is active.
            // VA-API first: mpv's auto order now tries Vulkan video first,
            // whose decoder faults on some AMD cards (RDNA4, 10-bit), and
            // with SVP's ROCm OpenCL loaded a GPU fault aborts the app.
            init.set_property("hwdec", HWDEC)?;
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
            // No video yet; the user's zoom still applies.
            return self.set("video-zoom", self.user_zoom());
        };
        let zoom = padding_zoom(
            (src_w as f64, src_h as f64),
            (out_w as f64, out_h as f64),
            (window_width as f64, window_height as f64),
        );
        self.mpv
            .set_property("video-zoom", zoom + self.user_zoom())
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
    /// `audio_file`: a separate audio stream to play with it (web trailers).
    pub fn load_at(
        self,
        target: &str,
        start_seconds: f64,
        subtitle_files: &[&str],
        audio_file: Option<&str>,
    ) -> Result<()> {
        // mpv ≥ 0.38 takes the playlist index before the per-file options.
        self.command(
            "loadfile",
            &[
                target,
                "replace",
                "-1",
                &file_options(start_seconds, subtitle_files, audio_file),
            ],
        )
    }

    /// Queues `target` after the current file (gapless with
    /// `prefetch-playlist`).
    pub fn append(self, target: &str) -> Result<()> {
        self.command("loadfile", &[target, "append"])
    }

    pub fn playlist_remove(self, index: i64) -> Result<()> {
        self.command("playlist-remove", &[&index.to_string()])
    }

    /// Music mode: no video track (cover art isn't decoded as video), the
    /// next track preloaded for gapless playback.
    pub fn set_audio_only(self, audio_only: bool) -> Result<()> {
        self.set("vid", if audio_only { "no" } else { "auto" })?;
        self.set(
            "audio-display",
            if audio_only { "no" } else { "embedded-first" },
        )?;
        self.set("prefetch-playlist", audio_only)
    }

    pub fn set_pause(self, paused: bool) -> Result<()> {
        self.set("pause", paused)
    }

    pub fn set_loop_file(self, on: bool) -> Result<()> {
        self.set("loop-file", if on { "inf" } else { "no" })
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

    /// Exposes (or closes) `socket`, the IPC path SVP Manager attaches to
    /// (its default is /tmp/mpvsocket). Turning it off also drops the
    /// filter SVP already added; turning it on lets SVP Manager find us.
    pub fn set_svp(self, socket: &str, enabled: bool) -> Result<()> {
        if enabled {
            self.set("input-ipc-server", socket)
        } else {
            // Clearing the option only closes the listening socket; mpv
            // keeps serving clients already connected, so SVP Manager
            // would stay attached and re-add its filter on the next file.
            self.set("input-ipc-server", "")?;
            let dropped = disconnect_ipc_clients(socket);
            if dropped > 0 {
                tracing::info!("disconnected {dropped} SVP client(s)");
            }
            // Not an error when SVP never attached.
            let _ = self.command("vf", &["remove", SVP_FILTER_LABEL]);
            Ok(())
        }
    }

    pub fn bar_fill(self) -> compositor::BarFill {
        match BAR_FILL.load(std::sync::atomic::Ordering::Relaxed) {
            1 => compositor::BarFill::Blur,
            2 => compositor::BarFill::Glow,
            _ => compositor::BarFill::Off,
        }
    }

    /// While a fill is on, subtitles stay inside the picture (mpv would
    /// otherwise place some in the bars, under the fill).
    pub fn set_bar_fill(self, fill: compositor::BarFill) -> Result<()> {
        let value = match fill {
            compositor::BarFill::Off => 0,
            compositor::BarFill::Blur => 1,
            compositor::BarFill::Glow => 2,
        };
        BAR_FILL.store(value, std::sync::atomic::Ordering::Relaxed);
        self.set("sub-use-margins", fill == compositor::BarFill::Off)
    }

    /// Where the picture sits in the render target, if there are bars.
    pub fn video_rect(self) -> Option<compositor::VideoRect> {
        let get = |key: &str| {
            self.mpv
                .get_property::<i64>(&format!("osd-dimensions/{key}"))
                .ok()
        };
        compositor::VideoRect::from_osd(
            get("w")?,
            get("h")?,
            get("ml")?,
            get("mr")?,
            get("mt")?,
            get("mb")?,
        )
    }

    pub fn set_aspect(self, aspect: Aspect) -> Result<()> {
        let (panscan, keepaspect, override_) = aspect.mpv_properties();
        self.set("panscan", panscan)?;
        self.set("keepaspect", keepaspect)?;
        self.set("video-aspect-override", override_)
    }

    pub fn user_zoom(self) -> f64 {
        f64::from_bits(USER_ZOOM.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Takes effect at the next `fit_to_window`.
    pub fn set_user_zoom(self, zoom: f64) {
        USER_ZOOM.store(
            zoom.clamp(-2.0, 2.0).to_bits(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Whether `set_smooth_motion(true)` is in effect.
    pub fn smooth_motion(self) -> bool {
        SMOOTH_MOTION.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// mpv's own frame blending to the display rate: smoother motion than
    /// plain playback, though not real interpolation like SVP.
    pub fn set_smooth_motion(self, enabled: bool) -> Result<()> {
        SMOOTH_MOTION.store(enabled, std::sync::atomic::Ordering::Relaxed);
        let (interpolation, video_sync) = if enabled {
            ("yes", "display-resample")
        } else {
            ("no", "audio")
        };
        self.set("interpolation", interpolation)?;
        self.set("video-sync", video_sync)?;
        if enabled {
            self.set("tscale", "oversample")?;
        }
        Ok(())
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

/// Shuts down every socket in this process whose local address is
/// `path`: the IPC connections mpv accepted on it. mpv's client thread
/// then sees EOF and drops the client. Returns how many were shut down.
fn disconnect_ipc_clients(path: &str) -> usize {
    let Ok(fds) = std::fs::read_dir("/proc/self/fd") else {
        return 0;
    };
    fds.filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<libc::c_int>().ok())
        .filter(|&fd| unix_socket_path(fd).as_deref() == Some(path))
        // SAFETY: shutdown on a socket fd we own; mpv's thread still owns
        // closing it, which shutdown doesn't do.
        .filter(|&fd| unsafe { libc::shutdown(fd, libc::SHUT_RDWR) } == 0)
        .count()
}

/// The local path of a Unix socket fd, if it is one.
fn unix_socket_path(fd: libc::c_int) -> Option<String> {
    // SAFETY: zeroed sockaddr_un is valid; getsockname writes at most `len` bytes.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
    let status = unsafe { libc::getsockname(fd, (&raw mut addr).cast(), &mut len) };
    if status != 0 || addr.sun_family != libc::AF_UNIX as libc::sa_family_t {
        return None;
    }
    let bytes: Vec<u8> = addr
        .sun_path
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8(bytes).ok()
}

/// Per-file `loadfile` options. Values use mpv's `%len%` quoting, since
/// URLs can contain the `,` and `=` that separate options.
fn file_options(start_seconds: f64, subtitle_files: &[&str], audio_file: Option<&str>) -> String {
    let mut options = vec![format!("start={start_seconds:.3}")];
    for file in subtitle_files {
        options.push(format!("sub-files-append=%{}%{file}", file.len()));
    }
    // %len% quoting: these URLs have commas of their own.
    if let Some(file) = audio_file {
        options.push(format!("audio-files-append=%{}%{file}", file.len()));
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
        ("playlist-pos", PropertyData::Int64(pos)) => Some(PlayerEvent::PlaylistPos(*pos)),
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
    fn finds_and_disconnects_unix_sockets_by_path() {
        use std::os::unix::net::{UnixListener, UnixStream};
        let path = std::env::temp_dir().join(format!("embyclientplus-ipc-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let client = UnixStream::connect(&path).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        drop(listener);
        let dropped = super::disconnect_ipc_clients(path.to_str().unwrap());
        assert_eq!(dropped, 1);
        // The far end sees EOF.
        let mut buf = [0u8; 1];
        assert_eq!(std::io::Read::read(&mut &client, &mut buf).unwrap(), 0);
        drop(accepted);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn aspect_modes_round_trip_and_map_to_mpv() {
        use super::Aspect;
        for aspect in Aspect::ALL {
            assert_eq!(Aspect::from_name(aspect.name()), Some(aspect));
        }
        assert_eq!(Aspect::Fill.mpv_properties(), (1.0, true, "no"));
        assert_eq!(Aspect::Stretch.mpv_properties(), (0.0, false, "no"));
        assert_eq!(Aspect::Force21x9.mpv_properties().2, "2.39:1");
        assert_eq!(Aspect::from_name("nonsense"), None);
    }

    #[test]
    fn file_options_quote_subtitle_urls() {
        assert_eq!(
            file_options(12.5, &["http://s/a.srt?x=1,2"], None),
            "start=12.500,sub-files-append=%20%http://s/a.srt?x=1,2"
        );
        assert_eq!(file_options(0.0, &[], None), "start=0.000");
        assert_eq!(
            file_options(0.0, &[], Some("http://a/b,c")),
            "start=0.000,audio-files-append=%12%http://a/b,c"
        );
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
