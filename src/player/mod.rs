pub mod video_area;

use std::sync::OnceLock;

use anyhow::{Result, anyhow};
use libmpv2::events::Event;
use libmpv2::{Format, Mpv};

/// Fixed path SVP Manager looks for (see `~/SVP4/mpv/mpv.conf`); it attaches
/// here and injects its vapoursynth filter over JSON IPC.
const SVP_IPC_SOCKET: &str = "/tmp/mpvsocket";

/// Display size before and after the filter chain; they differ once SVP's
/// black-bar lighting pads the frame.
const GEOMETRY_PROPERTIES: [&str; 4] = [
    "video-params/dw",
    "video-params/dh",
    "video-out-params/dw",
    "video-out-params/dh",
];

/// Called from the mpv event thread whenever a geometry property changes.
static GEOMETRY_CHANGED: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

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
            init.set_property("terminal", "yes")?;
            init.set_property("input-ipc-server", SVP_IPC_SOCKET)?;
            // Mirrors SVP's own [svp] mpv.conf profile: vapoursynth needs
            // decoded frames in system memory, and frame-dropping hr-seeks
            // desync audio once the filter is active.
            init.set_property("hwdec", "auto-copy")?;
            init.set_property("hr-seek-framedrop", "no")?;
            Ok(())
        })
        .map_err(|e| anyhow!("failed to initialize libmpv: {e:?}"))?;

        // One instance for the app's whole lifetime: render contexts borrow
        // it, and the event thread needs it too.
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));
        for (id, name) in (1..).zip(GEOMETRY_PROPERTIES) {
            mpv.observe_property(name, Format::Int64, id)
                .map_err(|e| anyhow!("failed to observe {name}: {e:?}"))?;
        }
        spawn_event_thread(mpv);
        Ok(Self { mpv })
    }

    pub(crate) fn mpv(self) -> &'static Mpv {
        self.mpv
    }

    /// Registers the callback run (on mpv's event thread) when the video's
    /// pre- or post-filter size changes. Only the first registration sticks.
    pub fn on_geometry_change(self, callback: impl Fn() + Send + Sync + 'static) {
        let _ = GEOMETRY_CHANGED.set(Box::new(callback));
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

    pub fn toggle_pause(self) -> Result<()> {
        self.command("cycle", &["pause"])
    }

    pub fn seek_relative(self, seconds: i32) -> Result<()> {
        self.command("seek", &[&seconds.to_string(), "relative"])
    }

    pub fn quit(self) -> Result<()> {
        self.command("quit", &[])
    }

    fn command(self, name: &str, args: &[&str]) -> Result<()> {
        self.mpv
            .command(name, args)
            .map_err(|e| anyhow!("mpv command {name} failed: {e:?}"))
    }
}

fn spawn_event_thread(mpv: &'static Mpv) {
    std::thread::spawn(move || {
        loop {
            match mpv.wait_event(-1.0) {
                Some(Ok(Event::Shutdown)) => break,
                Some(Ok(Event::EndFile(_))) => tracing::info!("playback ended"),
                Some(Ok(Event::PropertyChange { .. })) => {
                    if let Some(callback) = GEOMETRY_CHANGED.get() {
                        callback();
                    }
                }
                Some(Err(e)) => tracing::warn!("mpv event error: {e:?}"),
                _ => {}
            }
        }
    });
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
    use super::padding_zoom;

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
