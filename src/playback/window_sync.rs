use anyhow::{Context, Result};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ConfigureWindowAux, ConnectionExt, CreateWindowAux, EventMask, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

/// A bare X11 window mpv gets reparented into via `--wid`. Confirmed
/// working in the M1 spike: `override_redirect` makes it borderless with
/// no window-manager decoration, and `--gpu-context=x11vk` (never
/// `auto`, which silently picks `waylandvk` under this Wayland session
/// and renders black) is required for mpv to actually draw into it — see
/// NOTES.md.
///
/// Requires the whole app to be running under XWayland
/// (`GDK_BACKEND=x11`) — a native-Wayland main window cannot host this
/// at all, since reparenting only works within the same X11 display
/// (also see NOTES.md).
pub struct ContainerWindow {
    pub conn: RustConnection,
    pub window: Window,
}

impl ContainerWindow {
    pub fn create(x: i16, y: i16, width: u16, height: u16) -> Result<ContainerWindow> {
        let (conn, screen_num) =
            x11rb::connect(None).context("failed to connect to the X11 display")?;
        let window = conn
            .generate_id()
            .context("failed to generate an X11 window id")?;
        {
            let screen = &conn.setup().roots[screen_num];
            let aux = CreateWindowAux::new()
                .background_pixel(screen.black_pixel)
                .override_redirect(1)
                .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY);
            conn.create_window(
                screen.root_depth,
                window,
                screen.root,
                x,
                y,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &aux,
            )
            .context("failed to create the mpv container window")?
            .check()
            .context("X11 server rejected container window creation")?;
        }
        conn.map_window(window)
            .context("failed to map the mpv container window")?
            .check()
            .context("X11 server rejected mapping the container window")?;
        conn.flush().context("failed to flush X11 connection")?;
        Ok(ContainerWindow { conn, window })
    }

    /// Moves/resizes the window to follow the main app window (M10 —
    /// this was a fixed placeholder before, see NOTES.md).
    pub fn reconfigure(&self, x: i32, y: i32, width: u32, height: u32) -> Result<()> {
        self.conn
            .configure_window(
                self.window,
                &ConfigureWindowAux::new()
                    .x(x)
                    .y(y)
                    .width(width)
                    .height(height),
            )
            .context("failed to reconfigure the mpv container window")?
            .check()
            .context("X11 server rejected reconfiguring the container window")?;
        self.conn
            .flush()
            .context("failed to flush X11 connection")?;
        Ok(())
    }

    pub fn destroy(&self) -> Result<()> {
        self.conn
            .destroy_window(self.window)
            .context("failed to destroy the mpv container window")?
            .check()
            .context("X11 server rejected destroying the container window")?;
        self.conn
            .flush()
            .context("failed to flush X11 connection")?;
        Ok(())
    }
}
