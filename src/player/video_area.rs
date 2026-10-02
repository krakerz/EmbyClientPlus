use std::cell::{Cell, RefCell};
use std::ffi::{CString, c_char, c_void};
use std::rc::Rc;

use anyhow::{Context, Result, anyhow};
use gtk::glib;
use gtk::prelude::*;
use libloading::Library;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};

use super::Player;

const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;

type GetProcAddressFn = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type GetIntegervFn = unsafe extern "C" fn(u32, *mut i32);

/// GTK4 creates its GL contexts through EGL on both Wayland and X11, so
/// eglGetProcAddress resolves every GL entry point mpv needs.
struct GlLoader {
    _lib: Library,
    get_proc_address: GetProcAddressFn,
}

impl GlLoader {
    fn load() -> Result<Self> {
        // SAFETY: libEGL has no unsound initializers; GTK already loaded it.
        let lib = unsafe { Library::new("libEGL.so.1") }.context("failed to load libEGL")?;
        // SAFETY: eglGetProcAddress has exactly this signature; the copied
        // pointer stays valid because the library is kept alive alongside it.
        let get_proc_address = unsafe { *lib.get::<GetProcAddressFn>(b"eglGetProcAddress\0")? };
        Ok(Self {
            _lib: lib,
            get_proc_address,
        })
    }

    fn proc_address(&self, name: &str) -> *mut c_void {
        match CString::new(name) {
            // SAFETY: valid NUL-terminated name, called with a GL context current.
            Ok(name) => unsafe { (self.get_proc_address)(name.as_ptr()) },
            Err(_) => std::ptr::null_mut(),
        }
    }
}

fn get_proc_address(loader: &GlLoader, name: &str) -> *mut c_void {
    loader.proc_address(name)
}

#[derive(Default)]
struct RenderState {
    context: RefCell<Option<RenderContext<'static>>>,
    get_integerv: Cell<Option<GetIntegervFn>>,
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
                if let Err(e) = context.render::<GlLoader>(
                    fbo,
                    area.width() * scale,
                    area.height() * scale,
                    true,
                ) {
                    tracing::warn!("mpv render failed: {e:?}");
                }
            }
            glib::Propagation::Stop
        }
    });

    // The render context must go while its GL context is still current, or
    // mpv frees GL objects against a dead context on window close.
    area.connect_unrealize(move |area| {
        area.make_current();
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
