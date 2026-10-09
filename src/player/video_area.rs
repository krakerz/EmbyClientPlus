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

    let mut context = player
        .mpv()
        .create_render_context([
            RenderParam::ApiType(RenderParamApiType::OpenGl),
            RenderParam::InitParams(OpenGLInitParams {
                get_proc_address,
                ctx: loader,
            }),
        ])
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
