use anyhow::{Context, Result, bail};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    ColormapAlloc, ConfigureWindowAux, ConnectionExt, CreateGCAux, CreateWindowAux, EventMask,
    ImageFormat, VisualClass, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

/// Top-left, fixed-size "back to library" click target — a plain
/// rectangle for now, not a polished labeled button (that's later
/// visual polish, not this milestone's job); see `mod.rs`'s
/// `draw_back_button` for how it's actually painted into the frame.
pub const BACK_BUTTON: (i16, i16, u16, u16) = (0, 0, 80, 50);

/// Previous/next-episode click targets, next to [`BACK_BUTTON`] in the
/// same always-visible top-left row — navigation controls, grouped
/// separately from the auto-hiding seek-bar OSD and the bottom-right
/// quick-toggle buttons. Only drawn/hit-tested when a sibling episode
/// actually exists (`mod.rs`'s playback metadata resolution).
pub const PREV_BUTTON: (i16, i16, u16, u16) = (85, 0, 70, 50);
pub const NEXT_BUTTON: (i16, i16, u16, u16) = (160, 0, 70, 50);

/// A borderless, click-through-*intended* (actual input shaping is M10's
/// job, once there are real controls that need mixed clickable/
/// transparent regions) window with a true 32-bit ARGB visual, stacked
/// above the mpv container window, for compositing subtitle bitmaps
/// (and later, overlay controls) with real per-pixel transparency —
/// requires a compositing window manager (KWin here) to actually honor
/// the alpha channel.
/// Result of one [`OverlayWindow::poll_events`] drain.
pub struct PolledEvents {
    pub clicks: Vec<(i16, i16)>,
    pub motion: bool,
}

pub struct OverlayWindow {
    conn: RustConnection,
    window: Window,
    gc: x11rb::protocol::xproto::Gcontext,
    width: u16,
    height: u16,
}

impl OverlayWindow {
    pub fn create(x: i16, y: i16, width: u16, height: u16) -> Result<OverlayWindow> {
        let (conn, screen_num) =
            x11rb::connect(None).context("failed to connect to the X11 display")?;
        let (visual_id, depth) = {
            let screen = &conn.setup().roots[screen_num];
            find_argb_visual(screen).context(
                "no 32-bit TrueColor (ARGB) visual available — compositor support required",
            )?
        };

        let window = conn
            .generate_id()
            .context("failed to generate an X11 window id")?;
        let colormap = conn
            .generate_id()
            .context("failed to generate a colormap id")?;
        {
            let screen = &conn.setup().roots[screen_num];
            conn.create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual_id)
                .context("failed to create a colormap for the ARGB visual")?
                .check()
                .context("X11 server rejected colormap creation")?;

            // Click detection only, no keyboard — this is an
            // override_redirect window, so it's never window-manager-
            // focused; explicitly stealing input focus for a keyboard
            // shortcut here would fight with mpv's own native keybinds
            // (space/arrows) on the container window. Deferred rather
            // than fought with in this milestone.
            let aux = CreateWindowAux::new()
                .colormap(colormap)
                .border_pixel(0)
                .override_redirect(1)
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::STRUCTURE_NOTIFY
                        | EventMask::BUTTON_PRESS
                        | EventMask::POINTER_MOTION,
                );
            conn.create_window(
                depth,
                window,
                screen.root,
                x,
                y,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                visual_id,
                &aux,
            )
            .context("failed to create the overlay window")?
            .check()
            .context("X11 server rejected overlay window creation")?;
        }
        conn.map_window(window)
            .context("failed to map the overlay window")?
            .check()
            .context("X11 server rejected mapping the overlay window")?;

        let gc = conn
            .generate_id()
            .context("failed to generate a graphics context id")?;
        conn.create_gc(gc, window, &CreateGCAux::new())
            .context("failed to create a graphics context")?
            .check()
            .context("X11 server rejected graphics context creation")?;

        conn.flush().context("failed to flush X11 connection")?;
        Ok(OverlayWindow {
            conn,
            window,
            gc,
            width,
            height,
        })
    }

    /// Paints a straight-alpha RGBA8 buffer (`width * height * 4` bytes,
    /// row-major, matching what `subtitles::SubtitleRenderer::render_rgba`
    /// produces) onto the whole window. X11's `ZPixmap` format expects
    /// BGRA byte order on this little-endian host (confirmed via the
    /// server's own `image_byte_order`/pixel layout — matches the common
    /// convention for 32-bit TrueColor visuals), so channels are
    /// reordered per pixel.
    pub fn paint(&self, rgba: &[u8]) -> Result<()> {
        let expected_len = self.width as usize * self.height as usize * 4;
        if rgba.len() != expected_len {
            bail!(
                "paint buffer length {} doesn't match window size {}x{} ({} expected)",
                rgba.len(),
                self.width,
                self.height,
                expected_len
            );
        }
        let mut bgra = vec![0u8; rgba.len()];
        let (src_chunks, _) = rgba.as_chunks::<4>();
        let (dst_chunks, _) = bgra.as_chunks_mut::<4>();
        for (src, dst) in src_chunks.iter().zip(dst_chunks.iter_mut()) {
            dst[0] = src[2]; // B
            dst[1] = src[1]; // G
            dst[2] = src[0]; // R
            dst[3] = src[3]; // A
        }

        self.conn
            .put_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                self.gc,
                self.width,
                self.height,
                0,
                0,
                0,
                32,
                &bgra,
            )
            .context("failed to paint the overlay window")?
            .check()
            .context("X11 server rejected painting the overlay window")?;
        self.conn
            .flush()
            .context("failed to flush X11 connection")?;
        Ok(())
    }

    /// Moves/resizes the window to follow the main app window (M10 —
    /// this was a fixed placeholder before, see NOTES.md). Updates the
    /// dimensions `paint` validates against too.
    pub fn reconfigure(&mut self, x: i32, y: i32, width: u32, height: u32) -> Result<()> {
        self.conn
            .configure_window(
                self.window,
                &ConfigureWindowAux::new()
                    .x(x)
                    .y(y)
                    .width(width)
                    .height(height),
            )
            .context("failed to reconfigure the overlay window")?
            .check()
            .context("X11 server rejected reconfiguring the overlay window")?;
        self.conn
            .flush()
            .context("failed to flush X11 connection")?;
        self.width = width as u16;
        self.height = height as u16;
        Ok(())
    }

    /// Drains pending input events (non-blocking): every click's
    /// coordinates (hit-testing against button rects is the caller's job
    /// in `playback::mod`'s `run_session`, since those rects depend on
    /// the overlay's current size), plus whether any pointer motion was
    /// seen at all this poll — used only to know *that* the user is
    /// active over the video, not where, so the OSD auto-hide deadline
    /// can reset.
    pub fn poll_events(&self) -> Result<PolledEvents> {
        let mut clicks = Vec::new();
        let mut motion = false;
        while let Some(event) = self
            .conn
            .poll_for_event()
            .context("failed to poll X11 events")?
        {
            match event {
                Event::ButtonPress(press) => clicks.push((press.event_x, press.event_y)),
                Event::MotionNotify(_) => motion = true,
                _ => {}
            }
        }
        Ok(PolledEvents { clicks, motion })
    }

    pub fn destroy(&self) -> Result<()> {
        self.conn
            .destroy_window(self.window)
            .context("failed to destroy the overlay window")?
            .check()
            .context("X11 server rejected destroying the overlay window")?;
        self.conn
            .flush()
            .context("failed to flush X11 connection")?;
        Ok(())
    }
}

fn find_argb_visual(
    screen: &x11rb::protocol::xproto::Screen,
) -> Option<(x11rb::protocol::xproto::Visualid, u8)> {
    for depth in &screen.allowed_depths {
        if depth.depth != 32 {
            continue;
        }
        for visual in &depth.visuals {
            if visual.class == VisualClass::TRUE_COLOR {
                return Some((visual.visual_id, depth.depth));
            }
        }
    }
    None
}
