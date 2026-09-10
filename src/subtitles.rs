use std::ffi::{CString, c_char, c_int, c_longlong};
use std::ptr;

use anyhow::{Context, Result, bail};

use crate::emby::EmbyClient;

// Hand-rolled FFI — the only published binding crate (`libass-sys`) is
// broken against this system's headers (its bindgen-generated code
// panics on an anonymous enum name). Signatures and the ASS_Image layout
// below are copied from the real installed header
// (`/usr/include/ass/ass.h`, libass 0.17.5) and verified against a real
// render (see NOTES.md) rather than assumed from memory — in particular
// `color`'s low byte is alpha, **inverted**: 0x00 is fully opaque, 0xFF
// is fully transparent (the classic VSFilter-compatibility quirk the
// header's doc comment alludes to but doesn't spell out).
#[repr(C)]
struct AssLibrary {
    _private: [u8; 0],
}
#[repr(C)]
struct AssRendererRaw {
    _private: [u8; 0],
}
#[repr(C)]
struct AssTrackRaw {
    _private: [u8; 0],
}

#[repr(C)]
struct AssImage {
    w: c_int,
    h: c_int,
    stride: c_int,
    bitmap: *mut u8,
    color: u32,
    dst_x: c_int,
    dst_y: c_int,
    next: *mut AssImage,
    #[allow(dead_code)]
    type_: c_int,
}

unsafe extern "C" {
    fn ass_library_init() -> *mut AssLibrary;
    fn ass_library_done(lib: *mut AssLibrary);
    fn ass_renderer_init(lib: *mut AssLibrary) -> *mut AssRendererRaw;
    fn ass_renderer_done(r: *mut AssRendererRaw);
    fn ass_set_frame_size(r: *mut AssRendererRaw, w: c_int, h: c_int);
    fn ass_set_fonts(
        r: *mut AssRendererRaw,
        default_font: *const c_char,
        default_family: *const c_char,
        dfp: c_int,
        config: *const c_char,
        update: c_int,
    );
    fn ass_read_memory(
        lib: *mut AssLibrary,
        buf: *mut c_char,
        bufsize: usize,
        codepage: *const c_char,
    ) -> *mut AssTrackRaw;
    fn ass_free_track(track: *mut AssTrackRaw);
    fn ass_render_frame(
        r: *mut AssRendererRaw,
        track: *mut AssTrackRaw,
        now: c_longlong,
        detect_change: *mut c_int,
    ) -> *mut AssImage;
}

/// Renders a text-based subtitle track (SRT/ASS/VTT — libass reads all
/// three) into RGBA frames on demand, independent of mpv's own subtitle
/// rendering. Composited into our own overlay window instead, which
/// sidesteps lsfg-vk's swapchain-level ghosting of subtitle text (see
/// project CLAUDE.md) — mpv is launched with `--sid=no` whenever this is
/// in use.
pub struct SubtitleRenderer {
    library: *mut AssLibrary,
    renderer: *mut AssRendererRaw,
    track: *mut AssTrackRaw,
    width: u32,
    height: u32,
}

// Raw pointers aren't Send by default, but libass has no implicit
// thread-affinity of its own (it's a pure C library with no globals tied
// to a specific thread) — safe as long as one `SubtitleRenderer` is only
// ever driven from one task at a time, which is how playback uses it.
unsafe impl Send for SubtitleRenderer {}

impl SubtitleRenderer {
    pub fn new(width: u32, height: u32, subtitle_content: &str) -> Result<Self> {
        unsafe {
            let library = ass_library_init();
            if library.is_null() {
                bail!("ass_library_init failed");
            }
            let renderer = ass_renderer_init(library);
            if renderer.is_null() {
                ass_library_done(library);
                bail!("ass_renderer_init failed");
            }
            ass_set_frame_size(renderer, width as c_int, height as c_int);
            let family =
                CString::new("sans-serif").context("subtitle renderer default font family")?;
            ass_set_fonts(renderer, ptr::null(), family.as_ptr(), 1, ptr::null(), 1);

            let mut buf = subtitle_content.as_bytes().to_vec();
            let codepage = CString::new("UTF-8").context("subtitle codepage string")?;
            let track = ass_read_memory(
                library,
                buf.as_mut_ptr() as *mut c_char,
                buf.len(),
                codepage.as_ptr(),
            );
            if track.is_null() {
                ass_renderer_done(renderer);
                ass_library_done(library);
                bail!("ass_read_memory failed to parse subtitle content");
            }

            Ok(SubtitleRenderer {
                library,
                renderer,
                track,
                width,
                height,
            })
        }
    }

    /// Updates the frame size — needed whenever the overlay window
    /// resizes (M10's geometry lockstep with the main window), since
    /// libass bakes the frame size into the renderer at creation and
    /// `render_rgba`'s canvas allocation must match the window's actual
    /// current size. Cheap — no track re-parsing, just tells libass
    /// (and our own canvas allocation) about the new dimensions.
    pub fn resize(&mut self, width: u32, height: u32) {
        unsafe {
            ass_set_frame_size(self.renderer, width as c_int, height as c_int);
        }
        self.width = width;
        self.height = height;
    }

    /// Renders the subtitle frame for `time_ms` (from mpv's `time-pos`)
    /// into a straight-alpha RGBA8 buffer, `width * height * 4` bytes,
    /// row-major, transparent where nothing is drawn.
    pub fn render_rgba(&self, time_ms: i64) -> Vec<u8> {
        let mut canvas = vec![0u8; (self.width * self.height * 4) as usize];
        unsafe {
            let mut change: c_int = 0;
            let mut image_ptr = ass_render_frame(self.renderer, self.track, time_ms, &mut change);
            while !image_ptr.is_null() {
                let img = &*image_ptr;
                if img.w > 0 && img.h > 0 && !img.bitmap.is_null() {
                    composite_image(&mut canvas, self.width, self.height, img);
                }
                image_ptr = img.next;
            }
        }
        canvas
    }
}

impl Drop for SubtitleRenderer {
    fn drop(&mut self) {
        unsafe {
            ass_free_track(self.track);
            ass_renderer_done(self.renderer);
            ass_library_done(self.library);
        }
    }
}

/// Painter's-algorithm alpha-over blend of one `ASS_Image` onto the
/// canvas — images must be composited in list order (libass's own
/// documented requirement) since later images can overlay earlier ones
/// (e.g. glyph fill over its own outline).
fn composite_image(canvas: &mut [u8], canvas_w: u32, canvas_h: u32, img: &AssImage) {
    let r = (img.color >> 24) & 0xFF;
    let g = (img.color >> 16) & 0xFF;
    let b = (img.color >> 8) & 0xFF;
    let image_alpha = 255 - (img.color & 0xFF); // inverted, see module doc comment

    for y in 0..img.h {
        let dst_y = img.dst_y + y;
        if dst_y < 0 || dst_y as u32 >= canvas_h {
            continue;
        }
        for x in 0..img.w {
            let dst_x = img.dst_x + x;
            if dst_x < 0 || dst_x as u32 >= canvas_w {
                continue;
            }
            let src_off = (y as isize * img.stride as isize + x as isize) as usize;
            let coverage = unsafe { *img.bitmap.add(src_off) } as u32;
            let alpha = coverage * image_alpha / 255;
            if alpha == 0 {
                continue;
            }

            let dst_off = ((dst_y as u32 * canvas_w + dst_x as u32) * 4) as usize;
            let dst_a = canvas[dst_off + 3] as u32;
            let remainder = dst_a * (255 - alpha) / 255;
            let out_a = alpha + remainder;
            if out_a == 0 {
                continue;
            }
            for (i, src_c) in [r, g, b].into_iter().enumerate() {
                let dst_c = canvas[dst_off + i] as u32;
                canvas[dst_off + i] = ((src_c * alpha + dst_c * remainder) / out_a).min(255) as u8;
            }
            canvas[dst_off + 3] = out_a.min(255) as u8;
        }
    }
}

/// Fetches a subtitle stream's raw content directly from Emby —
/// `DeliveryMethod` doesn't matter, this endpoint serves any stream
/// (embedded or external) as a standalone file regardless of how it was
/// originally delivered.
pub async fn fetch_subtitle_content(
    emby: &EmbyClient,
    item_id: &str,
    media_source_id: &str,
    stream_index: i32,
    format: &str,
) -> Result<String> {
    emby.get_subtitle_stream(item_id, media_source_id, stream_index, format)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_ASS: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 640\nPlayResY: 480\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,40,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,0,2,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.00,0:00:10.00,Default,,0,0,0,,Hello World\n";

    #[test]
    fn renders_nonempty_alpha_for_real_text() {
        let renderer = SubtitleRenderer::new(640, 480, TEST_ASS).unwrap();
        let frame = renderer.render_rgba(1000);
        assert_eq!(frame.len(), 640 * 480 * 4);
        let any_visible = frame.as_chunks::<4>().0.iter().any(|px| px[3] > 0);
        assert!(
            any_visible,
            "expected some visible (non-transparent) pixels for real text"
        );
    }

    #[test]
    fn empty_before_and_after_dialogue_range() {
        let renderer = SubtitleRenderer::new(640, 480, TEST_ASS).unwrap();
        let frame = renderer.render_rgba(20_000); // past the 10s end time
        let any_visible = frame.as_chunks::<4>().0.iter().any(|px| px[3] > 0);
        assert!(
            !any_visible,
            "expected no visible pixels once the dialogue's end time has passed"
        );
    }
}
