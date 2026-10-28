use std::cell::{Cell, RefCell};
use std::ffi::{CString, c_char, c_void};
use std::rc::Rc;

use anyhow::{Context, Result, anyhow};
use gtk::glib;
use gtk::prelude::*;
use libloading::Library;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};

use super::Player;
use super::compositor::{BarFill, Compositor};

const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

type GetProcAddressFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type GetIntegervFn = unsafe extern "C" fn(u32, *mut i32);

/// Resolves GL entry points for mpv in whatever GL GTK made current.
/// Linux: GTK4 creates its contexts through EGL on both Wayland and X11,
/// so eglGetProcAddress covers everything. Windows: wglGetProcAddress
/// for extensions and GL > 1.1, opengl32.dll's own exports for the rest.
/// macOS: the OpenGL framework exports every entry point directly.
struct GlLoader {
    lib: Library,
    get_proc_address: Option<GetProcAddressFn>,
}

#[cfg(target_os = "linux")]
const GL_LIBRARY: (&str, Option<&[u8]>) = ("libEGL.so.1", Some(b"eglGetProcAddress\0"));
#[cfg(windows)]
const GL_LIBRARY: (&str, Option<&[u8]>) = ("opengl32.dll", Some(b"wglGetProcAddress\0"));
#[cfg(target_os = "macos")]
const GL_LIBRARY: (&str, Option<&[u8]>) =
    ("/System/Library/Frameworks/OpenGL.framework/OpenGL", None);

impl GlLoader {
    fn load() -> Result<Self> {
        let (name, getter) = GL_LIBRARY;
        // SAFETY: the system GL library has no unsound initializers; GTK
        // already loaded it.
        let lib =
            unsafe { Library::new(name) }.with_context(|| format!("failed to load {name}"))?;
        let get_proc_address = match getter {
            // SAFETY: egl/wglGetProcAddress have exactly this signature; the
            // copied pointer stays valid as the library is kept alongside it.
            Some(symbol) => Some(unsafe { *lib.get::<GetProcAddressFn>(symbol)? }),
            None => None,
        };
        Ok(Self {
            lib,
            get_proc_address,
        })
    }

    fn proc_address(&self, name: &str) -> *mut c_void {
        let Ok(name) = CString::new(name) else {
            return std::ptr::null_mut();
        };
        if let Some(get) = self.get_proc_address {
            // SAFETY: valid NUL-terminated name, called with a GL context current.
            let address = unsafe { get(name.as_ptr()) };
            // wglGetProcAddress signals "not here" with 0 but also 1, 2, 3 or -1.
            if !matches!(address as isize, -1..=3) || cfg!(not(windows)) {
                return address;
            }
        }
        // SAFETY: looking up an exported symbol; only its address is used.
        unsafe { self.lib.get::<*mut c_void>(name.as_bytes_with_nul()) }
            .map(|symbol| *symbol)
            .unwrap_or(std::ptr::null_mut())
    }
}

fn get_proc_address(loader: &GlLoader, name: &str) -> *mut c_void {
    loader.proc_address(name)
}

#[derive(Default)]
struct RenderState {
    context: RefCell<Option<RenderContext<'static>>>,
    get_integerv: Cell<Option<GetIntegervFn>>,
    /// Draws the bar fill; `None` if its shaders couldn't be built.
    compositor: RefCell<Option<Compositor>>,
    /// mpv drew a frame GTK hasn't presented yet (see `report_swap`).
    rendered: Cell<bool>,
}

/// A GLArea that libmpv draws video frames into via its OpenGL render API.
pub fn new(player: Player) -> gtk::GLArea {
    let area = gtk::GLArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    let state = Rc::new(RenderState::default());

    // The owner also calls `refit` on `PlayerEvent::Geometry`.
    area.connect_resize(move |_, width, height| fit(player, width, height));

    area.connect_realize({
        let state = state.clone();
        move |area| {
            area.make_current();
            if let Some(err) = area.error() {
                tracing::error!("GLArea has no usable GL context: {err}");
                return;
            }
            match create_render_context(player, area) {
                Ok((context, get_integerv)) => {
                    state.get_integerv.set(Some(get_integerv));
                    state.context.replace(Some(context));
                }
                Err(e) => tracing::error!("failed to create mpv render context: {e:#}"),
            }
            match create_compositor(area) {
                Ok(compositor) => {
                    state.compositor.replace(Some(compositor));
                }
                Err(e) => tracing::warn!("bar fill unavailable: {e:#}"),
            }
            track_display_fps(player, area);
            // Tells mpv when each frame it drew went out, so display-synced
            // timing follows the real refresh. Once per drawn frame only:
            // mpv expects it consistently.
            if let Some(clock) = area.frame_clock() {
                let state = state.clone();
                clock.connect_after_paint(move |_| {
                    crate::player::diag::note_window_frame();
                    if state.rendered.replace(false)
                        && crate::player::diag::switches().report_swap
                        && let Some(context) = state.context.borrow().as_ref()
                    {
                        context.report_swap();
                        crate::player::diag::note_swap();
                    }
                });
            }
        }
    });

    area.connect_render({
        let state = state.clone();
        move |area, _| {
            if let (Some(context), Some(get_integerv)) =
                (state.context.borrow().as_ref(), state.get_integerv.get())
            {
                let mut fbo = 0;
                // SAFETY: valid GL entry point, GTK made the context current.
                unsafe { get_integerv(GL_DRAW_FRAMEBUFFER_BINDING, &mut fbo) };
                let scale = area.scale_factor();
                let (width, height) = (area.width() * scale, area.height() * scale);
                let fill = player.bar_fill();
                let mut compositor = state.compositor.borrow_mut();
                let rect = (fill != BarFill::Off)
                    .then(|| player.video_rect())
                    .flatten();
                match (compositor.as_mut(), rect) {
                    // Fill on and there are bars: render offscreen, then composite.
                    (Some(compositor), Some(rect)) => {
                        let result = compositor.scene_fbo(width, height).and_then(|scene| {
                            context
                                .render::<GlLoader>(scene, width, height, true)
                                .map_err(|e| format!("{e:?}"))?;
                            compositor.composite(fill, rect, fbo)
                        });
                        if let Err(e) = result {
                            tracing::warn!("bar fill failed: {e}");
                        }
                    }
                    _ => {
                        if let Err(e) = context.render::<GlLoader>(fbo, width, height, true) {
                            tracing::warn!("mpv render failed: {e:?}");
                        }
                    }
                }
                state.rendered.set(true);
                crate::player::diag::note_paint();
                if crate::smoke::pending() {
                    crate::smoke::frame(player, gl_info);
                }
            }
            glib::Propagation::Stop
        }
    });

    // The render context must go while its GL context is still current, or
    // mpv frees GL objects against a dead context on window close.
    area.connect_unrealize(move |area| {
        area.make_current();
        state.compositor.take();
        state.context.take();
    });

    area
}

/// Tells mpv the display rate (Preferences) when the window enters a
/// monitor; Automatic takes that monitor's refresh rate.
fn track_display_fps(player: Player, area: &gtk::GLArea) {
    fn apply(player: Player, monitor: &gtk::gdk::Monitor) {
        crate::player::diag::note_monitor(monitor.refresh_rate());
        crate::player::diag::apply_display_rate(player);
    }
    let Some(surface) = area.native().and_then(|native| native.surface()) else {
        return;
    };
    if let Some(monitor) = area.display().monitor_at_surface(&surface) {
        apply(player, &monitor);
    }
    surface.connect_enter_monitor(move |_, monitor| {
        apply(player, monitor);
    });
}

/// Re-applies the SVP padding fit after the video's geometry changed.
pub fn refit(player: Player, area: &gtk::GLArea) {
    fit(player, area.width(), area.height());
}

fn fit(player: Player, width: i32, height: i32) {
    if width > 0
        && height > 0
        && let Err(e) = player.fit_to_window(width, height)
    {
        tracing::warn!("{e:#}");
    }
}

/// (GL_RENDERER, GL_VERSION) of the current context, for the smoke test.
fn gl_info() -> (String, String) {
    type GetStringFn = unsafe extern "C" fn(u32) -> *const c_char;
    const GL_RENDERER: u32 = 0x1F01;
    const GL_VERSION: u32 = 0x1F02;
    let unknown = || "unknown".to_string();
    let Ok(loader) = GlLoader::load() else {
        return (unknown(), unknown());
    };
    let get_string = loader.proc_address("glGetString");
    if get_string.is_null() {
        return (unknown(), unknown());
    }
    // SAFETY: non-null pointer to glGetString, whose signature this matches.
    let get_string: GetStringFn = unsafe { std::mem::transmute(get_string) };
    let read = |name| {
        // SAFETY: GTK made the context current; glGetString returns a static
        // NUL-terminated string or null.
        let text = unsafe { get_string(name) };
        if text.is_null() {
            unknown()
        } else {
            unsafe { std::ffi::CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        }
    };
    (read(GL_RENDERER), read(GL_VERSION))
}

fn create_compositor(area: &gtk::GLArea) -> Result<Compositor> {
    let loader = GlLoader::load()?;
    // SAFETY: the GLArea's context is current (realize); the loader stays
    // valid for the function pointers glow copies out.
    let gl = unsafe {
        glow::Context::from_loader_function(|name| loader.proc_address(name) as *const _)
    };
    let es = area.context().is_some_and(|context| context.uses_es());
    Compositor::new(gl, es).map_err(|e| anyhow!(e))
}

/// mpv draws with GL; GTK's Vulkan renderer (the default on Linux since
/// 4.16) hands every video frame from GL over to Vulkan, which cost ~25% of
/// a core at 1080p24. One the user set wins. Call before GTK init.
pub fn prefer_gl_renderer() {
    if std::env::var_os("GSK_RENDERER").is_none() {
        // SAFETY: called first thing in main, while single-threaded.
        unsafe { std::env::set_var("GSK_RENDERER", "ngl") };
    }
}

/// GDK's Wayland or X11 display, which mpv's VA-API interop needs for
/// zero-copy decoding (without it only vaapi-copy works). Looked up at run
/// time: GTK may be built without either backend.
#[cfg(target_os = "linux")]
fn native_display<C>(display: &gtk::gdk::Display) -> Option<RenderParam<C>> {
    let (getter, wayland) = match display.type_().name() {
        "GdkWaylandDisplay" => (c"gdk_wayland_display_get_wl_display", true),
        "GdkX11Display" => (c"gdk_x11_display_get_xdisplay", false),
        _ => return None,
    };
    // SAFETY: looks up a symbol of the already loaded GTK library.
    let getter = unsafe { libc::dlsym(libc::RTLD_DEFAULT, getter.as_ptr()) };
    if getter.is_null() {
        return None;
    }
    // SAFETY: both getters take a GdkDisplay of their backend (checked
    // above) and return its native handle, valid as long as the display.
    let native = unsafe {
        let getter: unsafe extern "C" fn(*mut c_void) -> *mut c_void = std::mem::transmute(getter);
        getter(display.as_ptr().cast())
    };
    match (native.is_null(), wayland) {
        (true, _) => None,
        (false, true) => Some(RenderParam::WaylandDisplay(native)),
        (false, false) => Some(RenderParam::X11Display(native)),
    }
}

/// Windows/macOS interop finds its device through the GL context itself.
#[cfg(not(target_os = "linux"))]
fn native_display<C>(_: &gtk::gdk::Display) -> Option<RenderParam<C>> {
    None
}

fn create_render_context(
    player: Player,
    area: &gtk::GLArea,
) -> Result<(RenderContext<'static>, GetIntegervFn)> {
    let loader = GlLoader::load()?;
    let get_integerv = loader.proc_address("glGetIntegerv");
    if get_integerv.is_null() {
        return Err(anyhow!("glGetIntegerv not available"));
    }
    // SAFETY: non-null pointer to glGetIntegerv, whose signature this matches.
    let get_integerv: GetIntegervFn = unsafe { std::mem::transmute(get_integerv) };

    let mut params = vec![
        RenderParam::ApiType(RenderParamApiType::OpenGl),
        RenderParam::InitParams(OpenGLInitParams {
            get_proc_address,
            ctx: loader,
        }),
    ];
    params.extend(native_display(&area.display()));
    let mut context = player
        .mpv()
        .create_render_context(params)
        .map_err(|e| anyhow!("{e:?}"))?;

    // Fires on an mpv thread whenever a new frame is ready.
    let area = glib::SendWeakRef::from(area.downgrade());
    context.set_update_callback(move || {
        let area = area.clone();
        glib::MainContext::default().invoke(move || {
            if let Some(area) = area.upgrade() {
                area.queue_render();
            }
        });
    });

    Ok((context, get_integerv))
}
