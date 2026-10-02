//! Our own "fill the black bars" effect. mpv's libmpv renderer has no
//! blurred background (that's gpu-next only), so when a fill is on, mpv
//! draws into an offscreen texture and this composites the window:
//!
//! 1. the picture's rectangle (from mpv's `osd-dimensions`) is downsampled
//!    into a small texture and Gaussian-blurred;
//! 2. the bars get either the blurred picture scaled to cover the window
//!    ("blur") or the picture's edges extended outward and fading ("glow");
//! 3. the sharp picture, subtitles included, is copied on top.
//!
//! Works the same for side bars and top/bottom bars (e.g. 16:10 screens).

use std::num::NonZeroU32;

use glow::HasContext;

/// What fills the bars around the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BarFill {
    #[default]
    Off,
    Blur,
    Glow,
}

/// The picture's rectangle in the render target, in pixels, with the
/// origin at the top-left (as mpv reports it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoRect {
    pub width: f32,
    pub height: f32,
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
}

impl VideoRect {
    /// From mpv's `osd-dimensions` (w, h, ml, mr, mt, mb). `None` when the
    /// picture fills the target (nothing to fill) or there's no picture.
    pub fn from_osd(w: i64, h: i64, ml: i64, mr: i64, mt: i64, mb: i64) -> Option<Self> {
        let rect = VideoRect {
            width: w as f32,
            height: h as f32,
            left: ml as f32,
            right: mr as f32,
            top: mt as f32,
            bottom: mb as f32,
        };
        let picture_w = rect.width - rect.left - rect.right;
        let picture_h = rect.height - rect.top - rect.bottom;
        let has_bars = ml > 1 || mr > 1 || mt > 1 || mb > 1;
        (w > 0 && h > 0 && picture_w > 1.0 && picture_h > 1.0 && has_bars).then_some(rect)
    }

    fn picture_size(&self) -> (f32, f32) {
        (
            self.width - self.left - self.right,
            self.height - self.top - self.bottom,
        )
    }

    /// The picture in GL terms (origin bottom-left): x, y, w, h in pixels.
    fn gl_viewport(&self, target_w: i32, target_h: i32) -> (i32, i32, i32, i32) {
        let sx = target_w as f32 / self.width;
        let sy = target_h as f32 / self.height;
        let (pw, ph) = self.picture_size();
        (
            (self.left * sx).round() as i32,
            (self.bottom * sy).round() as i32,
            (pw * sx).round() as i32,
            (ph * sy).round() as i32,
        )
    }

    /// The picture as texture coordinates (origin bottom-left): u0, v0, u1, v1.
    fn uv(&self) -> [f32; 4] {
        [
            self.left / self.width,
            self.bottom / self.height,
            1.0 - self.right / self.width,
            1.0 - self.top / self.height,
        ]
    }
}

/// A texture with a framebuffer drawing into it.
struct Target {
    fbo: glow::Framebuffer,
    texture: glow::Texture,
    width: i32,
    height: i32,
}

pub struct Compositor {
    gl: glow::Context,
    copy: glow::Program,
    blur: glow::Program,
    fill: glow::Program,
    vao: glow::VertexArray,
    scene: Option<Target>,
    small: Option<[Target; 2]>,
}

/// Blur runs at this fraction of the picture size: cheap, and the
/// downsampling itself already softens a lot.
const BLUR_SCALE: f32 = 1.0 / 12.0;
const BLUR_PASSES: usize = 3;

const VERTEX: &str = r#"
out vec2 uv;
void main() {
    // A triangle covering the viewport; uv spans 0..1 across it.
    vec2 corner = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    uv = corner;
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// Copies a region (`src_rect`: u0, v0, u1, v1) of `tex` to the viewport.
const COPY: &str = r#"
in vec2 uv;
out vec4 color;
uniform sampler2D tex;
uniform vec4 src_rect;
void main() {
    color = texture(tex, mix(src_rect.xy, src_rect.zw, uv));
}
"#;

/// One direction of a 9-tap Gaussian.
const BLUR: &str = r#"
in vec2 uv;
out vec4 color;
uniform sampler2D tex;
uniform vec2 step_size;
void main() {
    vec4 sum = texture(tex, uv) * 0.227027;
    sum += texture(tex, uv + step_size * 1.3846) * 0.316216;
    sum += texture(tex, uv - step_size * 1.3846) * 0.316216;
    sum += texture(tex, uv + step_size * 3.2308) * 0.070270;
    sum += texture(tex, uv - step_size * 3.2308) * 0.070270;
    color = sum;
}
"#;

/// The bars: `mode` 1 = blurred picture covering the window, 2 = glow from
/// the picture's nearest edge, fading with distance.
const FILL: &str = r#"
in vec2 uv;
out vec4 color;
uniform sampler2D blurred;
uniform vec2 window;      // target size in pixels
uniform vec4 picture;     // x, y, w, h in pixels (origin bottom-left)
uniform int mode;
void main() {
    vec2 p = uv * window;
    vec2 lo = picture.xy;
    vec2 hi = picture.xy + picture.zw;
    vec3 c;
    if (mode == 1) {
        // Scale the blurred picture to cover the window, keeping its aspect.
        float k = max(window.x / picture.z, window.y / picture.w);
        vec2 q = (p - window * 0.5) / (picture.zw * k) + 0.5;
        c = texture(blurred, clamp(q, 0.0, 1.0)).rgb;
        float grey = dot(c, vec3(0.299, 0.587, 0.114));
        c = mix(vec3(grey), c, 0.8) * 0.45;
    } else {
        vec2 edge = clamp(p, lo, hi);
        vec2 q = (edge - lo) / picture.zw;
        c = texture(blurred, clamp(q, 0.0, 1.0)).rgb;
        float d = distance(p, edge);
        float reach = 0.22 * max(window.x, window.y);
        c *= 0.85 * exp(-d / reach * 2.2);
    }
    color = vec4(c, 1.0);
}
"#;

impl Compositor {
    /// Builds the GL programs. Call with the GLArea's context current.
    pub fn new(gl: glow::Context, es: bool) -> Result<Self, String> {
        let header = if es {
            "#version 300 es\nprecision highp float;\n"
        } else {
            "#version 330 core\n"
        };
        // SAFETY: a current GL context is required by the caller; all objects
        // are created and used on this thread only.
        unsafe {
            let copy = program(&gl, header, COPY)?;
            let blur = program(&gl, header, BLUR)?;
            let fill = program(&gl, header, FILL)?;
            let vao = gl.create_vertex_array()?;
            Ok(Compositor {
                gl,
                copy,
                blur,
                fill,
                vao,
                scene: None,
                small: None,
            })
        }
    }

    /// The framebuffer mpv should render the frame into, sized `w`×`h`.
    pub fn scene_fbo(&mut self, width: i32, height: i32) -> Result<i32, String> {
        if self
            .scene
            .as_ref()
            .is_none_or(|t| t.width != width || t.height != height)
        {
            if let Some(old) = self.scene.take() {
                self.delete(old);
            }
            // SAFETY: as in `new`.
            self.scene = Some(unsafe { target(&self.gl, width, height)? });
        }
        Ok(fbo_id(self.scene.as_ref().map(|t| t.fbo)))
    }

    /// Composites the scene (already rendered by mpv) into `out_fbo`.
    pub fn composite(
        &mut self,
        mode: BarFill,
        rect: VideoRect,
        out_fbo: i32,
    ) -> Result<(), String> {
        let Some(scene) = self.scene.as_ref() else {
            return Ok(());
        };
        let (w, h) = (scene.width, scene.height);
        let scene_texture = scene.texture;
        let (pw, ph) = rect.picture_size();
        let small_w = ((pw * BLUR_SCALE).round() as i32).clamp(16, 512);
        let small_h = ((ph * BLUR_SCALE).round() as i32).clamp(16, 512);
        if self
            .small
            .as_ref()
            .is_none_or(|[a, _]| a.width != small_w || a.height != small_h)
        {
            if let Some([a, b]) = self.small.take() {
                self.delete(a);
                self.delete(b);
            }
            // SAFETY: as in `new`.
            self.small = Some(unsafe {
                [
                    target(&self.gl, small_w, small_h)?,
                    target(&self.gl, small_w, small_h)?,
                ]
            });
        }
        let Some([a, b]) = self.small.as_ref() else {
            return Ok(());
        };
        let gl = &self.gl;
        let (vx, vy, vw, vh) = rect.gl_viewport(w, h);
        // SAFETY: as in `new`; every texture/framebuffer used here is ours
        // except `out_fbo`, GTK's current framebuffer.
        unsafe {
            gl.bind_vertex_array(Some(self.vao));
            gl.disable(glow::BLEND);
            gl.disable(glow::SCISSOR_TEST);
            gl.active_texture(glow::TEXTURE0);

            // 1. The picture only, downsampled.
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(a.fbo));
            gl.viewport(0, 0, small_w, small_h);
            gl.use_program(Some(self.copy));
            gl.bind_texture(glow::TEXTURE_2D, Some(scene_texture));
            set_i32(gl, self.copy, "tex", 0);
            set_vec4(gl, self.copy, "src_rect", rect.uv());
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            // 2. Blur back and forth between the two small targets.
            gl.use_program(Some(self.blur));
            set_i32(gl, self.blur, "tex", 0);
            for _ in 0..BLUR_PASSES {
                for (from, to, step) in [
                    (a, b, [1.0 / small_w as f32, 0.0]),
                    (b, a, [0.0, 1.0 / small_h as f32]),
                ] {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(to.fbo));
                    gl.bind_texture(glow::TEXTURE_2D, Some(from.texture));
                    let location = gl.get_uniform_location(self.blur, "step_size");
                    gl.uniform_2_f32(location.as_ref(), step[0], step[1]);
                    gl.draw_arrays(glow::TRIANGLES, 0, 3);
                }
            }

            // 3. Bars, then the sharp picture on top.
            let out = framebuffer(out_fbo);
            gl.bind_framebuffer(glow::FRAMEBUFFER, out);
            gl.viewport(0, 0, w, h);
            gl.use_program(Some(self.fill));
            gl.bind_texture(glow::TEXTURE_2D, Some(a.texture));
            set_i32(gl, self.fill, "blurred", 0);
            set_i32(
                gl,
                self.fill,
                "mode",
                if mode == BarFill::Blur { 1 } else { 2 },
            );
            let window = gl.get_uniform_location(self.fill, "window");
            gl.uniform_2_f32(window.as_ref(), w as f32, h as f32);
            set_vec4(
                gl,
                self.fill,
                "picture",
                [vx as f32, vy as f32, vw as f32, vh as f32],
            );
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            gl.viewport(vx, vy, vw, vh);
            gl.use_program(Some(self.copy));
            gl.bind_texture(glow::TEXTURE_2D, Some(scene_texture));
            set_i32(gl, self.copy, "tex", 0);
            set_vec4(gl, self.copy, "src_rect", rect.uv());
            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            // Leave GTK's state as it expects it.
            gl.viewport(0, 0, w, h);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
            gl.bind_vertex_array(None);
        }
        Ok(())
    }

    fn delete(&self, target: Target) {
        // SAFETY: deleting our own objects with the context current.
        unsafe {
            self.gl.delete_framebuffer(target.fbo);
            self.gl.delete_texture(target.texture);
        }
    }
}

impl Drop for Compositor {
    /// Only drop with the GL context current (see video_area's unrealize).
    fn drop(&mut self) {
        if let Some(scene) = self.scene.take() {
            self.delete(scene);
        }
        if let Some([a, b]) = self.small.take() {
            self.delete(a);
            self.delete(b);
        }
        // SAFETY: as above.
        unsafe {
            self.gl.delete_program(self.copy);
            self.gl.delete_program(self.blur);
            self.gl.delete_program(self.fill);
            self.gl.delete_vertex_array(self.vao);
        }
    }
}

fn framebuffer(id: i32) -> Option<glow::Framebuffer> {
    NonZeroU32::new(id as u32).map(glow::NativeFramebuffer)
}

fn fbo_id(fbo: Option<glow::Framebuffer>) -> i32 {
    fbo.map_or(0, |fbo| fbo.0.get() as i32)
}

unsafe fn set_i32(gl: &glow::Context, program: glow::Program, name: &str, value: i32) {
    // SAFETY: caller holds a current context and `program` is in use.
    unsafe {
        let location = gl.get_uniform_location(program, name);
        gl.uniform_1_i32(location.as_ref(), value);
    }
}

unsafe fn set_vec4(gl: &glow::Context, program: glow::Program, name: &str, v: [f32; 4]) {
    // SAFETY: as in `set_i32`.
    unsafe {
        let location = gl.get_uniform_location(program, name);
        gl.uniform_4_f32(location.as_ref(), v[0], v[1], v[2], v[3]);
    }
}

unsafe fn program(
    gl: &glow::Context,
    header: &str,
    fragment: &str,
) -> Result<glow::Program, String> {
    // SAFETY: caller holds a current context.
    unsafe {
        let program = gl.create_program()?;
        for (kind, source) in [
            (glow::VERTEX_SHADER, VERTEX),
            (glow::FRAGMENT_SHADER, fragment),
        ] {
            let shader = gl.create_shader(kind)?;
            gl.shader_source(shader, &format!("{header}{source}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let log = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                return Err(format!("shader didn't compile: {log}"));
            }
            gl.attach_shader(program, shader);
            gl.delete_shader(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            return Err(format!(
                "program didn't link: {}",
                gl.get_program_info_log(program)
            ));
        }
        Ok(program)
    }
}

unsafe fn target(gl: &glow::Context, width: i32, height: i32) -> Result<Target, String> {
    // SAFETY: caller holds a current context.
    unsafe {
        let texture = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            width,
            height,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        for (param, value) in [
            (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, param, value as i32);
        }
        let fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(texture),
            0,
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_texture(glow::TEXTURE_2D, None);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fbo);
            gl.delete_texture(texture);
            return Err(format!("offscreen framebuffer incomplete (0x{status:x})"));
        }
        Ok(Target {
            fbo,
            texture,
            width,
            height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_bars_and_top_bars_both_count() {
        // 16:9 video in a 21:9 window: side bars.
        let side = VideoRect::from_osd(2560, 1080, 320, 320, 0, 0).unwrap();
        assert_eq!(side.picture_size(), (1920.0, 1080.0));
        assert_eq!(side.gl_viewport(2560, 1080), (320, 0, 1920, 1080));
        // 16:9 video on a 16:10 handheld (1920×1200): top/bottom bars.
        let top = VideoRect::from_osd(1920, 1200, 0, 0, 60, 60).unwrap();
        assert_eq!(top.gl_viewport(1920, 1200), (0, 60, 1920, 1080));
        let [u0, v0, u1, v1] = top.uv();
        assert_eq!((u0, u1), (0.0, 1.0));
        assert!((v0 - 0.05).abs() < 1e-6 && (v1 - 0.95).abs() < 1e-6);
    }

    #[test]
    fn nothing_to_fill_without_bars_or_picture() {
        assert_eq!(VideoRect::from_osd(1920, 1080, 0, 0, 0, 0), None);
        assert_eq!(VideoRect::from_osd(0, 0, 0, 0, 0, 0), None);
        assert_eq!(VideoRect::from_osd(1920, 1080, 960, 960, 0, 0), None);
    }

    #[test]
    fn uneven_bars_map_to_gl_origin() {
        // mpv's top margin is GL's distance from the top, so y = bottom.
        let rect = VideoRect::from_osd(1000, 800, 0, 0, 100, 300).unwrap();
        assert_eq!(rect.gl_viewport(1000, 800), (0, 300, 1000, 400));
    }
}
