use std::path::PathBuf;

use anyhow::{Context, Result, bail};

/// Generates a real motion-interpolation script (confirmed working
/// end-to-end in the M4 spike: `mv.Super` → `mv.Analyse` ×2 →
/// `mv.FlowFPS`) for mpv's `--vf=vapoursynth` filter. `mpv` feeds the
/// clip in as the `video_in` global; it carries no frame-rate metadata
/// by default, so `AssumeFPS` is required before `FlowFPS` will accept
/// it (confirmed in the M4 spike — omitting it fails with "must have a
/// frame rate").
pub fn generate_vpy_script(source_fps_num: u32, source_fps_den: u32, multiplier: u32) -> String {
    let output_fps_num = source_fps_num * multiplier;
    format!(
        r#"import vapoursynth as vs
core = vs.core

clip = video_in
clip = core.resize.Bicubic(clip, format=vs.YUV420P8)
clip = core.std.AssumeFPS(clip, fpsnum={source_fps_num}, fpsden={source_fps_den})

super_clip = core.mv.Super(clip, pel=2)
backward = core.mv.Analyse(super_clip, isb=True, overlap=4, blksize=16)
forward = core.mv.Analyse(super_clip, isb=False, overlap=4, blksize=16)

interpolated = core.mv.FlowFPS(clip, super_clip, backward, forward, num={output_fps_num}, den={source_fps_den})
interpolated.set_output()
"#
    )
}

/// Writes the generated script to a per-session temp file mpv can load
/// via `--vf=vapoursynth=<path>`.
pub fn write_vpy_script(
    session_dir: &std::path::Path,
    source_fps_num: u32,
    source_fps_den: u32,
    multiplier: u32,
) -> Result<PathBuf> {
    std::fs::create_dir_all(session_dir)
        .with_context(|| format!("failed to create {}", session_dir.display()))?;
    let path = session_dir.join("interp.vpy");
    let script = generate_vpy_script(source_fps_num, source_fps_den, multiplier);
    std::fs::write(&path, script).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

/// Distro mpv builds don't have `--enable-vapoursynth` compiled in
/// (confirmed in NOTES.md) — this path needs a custom-built mpv, which
/// isn't bundled by the app yet (that's M12 packaging). Checks a couple
/// of known-real locations as a pragmatic placeholder rather than
/// silently falling back to a build that will reject the filter.
pub fn resolve_vapoursynth_mpv_binary() -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(&home).join("SVP4/mpv/mpv"));
    }
    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!(
        "no vapoursynth-enabled mpv build found — SVP mode needs a custom mpv build \
         (`-Dvapoursynth=enabled`), not bundled by the app yet (M12)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_assumes_fps_before_flowfps() {
        let script = generate_vpy_script(24, 1, 2);
        let assume_pos = script.find("AssumeFPS").unwrap();
        let flowfps_pos = script.find("FlowFPS").unwrap();
        assert!(assume_pos < flowfps_pos);
        assert!(script.contains("fpsnum=24"));
        assert!(script.contains("num=48"));
    }
}
