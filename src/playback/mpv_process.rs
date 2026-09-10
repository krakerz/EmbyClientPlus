use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use anyhow::{Context, Result};

use crate::config::{FrameGenBackend, FrameGenSettings};
use crate::frame_gen::{lsfg_vk, svp};

pub struct MpvLaunch {
    pub child: Child,
}

pub struct SpawnArgs<'a> {
    pub backend: FrameGenBackend,
    pub frame_gen_settings: &'a FrameGenSettings,
    pub session_dir: &'a Path,
    pub container_window: u32,
    pub socket_path: &'a Path,
    pub stream_url: &'a str,
    /// Set when we're rendering subtitles ourselves onto a separate
    /// overlay window (see `subtitles.rs`) — avoids mpv also drawing
    /// them onto the same frames.
    pub disable_native_subs: bool,
    /// Set when this spawn is resuming an in-progress session (a quality
    /// or frame-gen quick-toggle restarting the mpv child in place) —
    /// appends `--start=` so playback picks up where it left off instead
    /// of restarting from 0.
    pub resume_seconds: Option<f64>,
}

fn which_mpv() -> Result<PathBuf> {
    let output = Command::new("which")
        .arg("mpv")
        .output()
        .context("failed to run `which mpv`")?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        anyhow::bail!("mpv not found in PATH");
    }
    Ok(PathBuf::from(path))
}

/// Spawns mpv embedded into `container_window`, configured for the
/// requested frame-gen backend. All flags here are the exact recipe
/// confirmed working across the M1/M2/M4 spikes — never rely on `auto`
/// for `--gpu-context` (see `window_sync`), and always pass
/// `--no-config` (this machine's own personal mpv.conf otherwise
/// interferes with IPC socket binding — see NOTES.md).
pub fn spawn(args: SpawnArgs) -> Result<MpvLaunch> {
    let mut binary = which_mpv()?;
    let mut extra_args: Vec<String> = Vec::new();

    match args.backend {
        FrameGenBackend::Off => {}
        FrameGenBackend::LsfgVk => {
            let lsfg = &args.frame_gen_settings.lsfg_vk;
            match lsfg_vk::ensure_active_in_registered(lsfg.multiplier, lsfg.flow_scale) {
                Ok(()) => {
                    binary = lsfg_vk::ensure_process_symlink(&binary)?;
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "lsfg-vk unavailable, falling back to plain playback"
                    );
                }
            }
        }
        FrameGenBackend::Svp => match svp::resolve_vapoursynth_mpv_binary() {
            Ok(vs_mpv) => {
                // Source fps isn't known yet at spawn time in this first
                // cut — 24/1 is a placeholder until PlaybackInfo's media
                // stream metadata is threaded through. AssumeFPS being
                // wrong just mis-times interpolation, it doesn't error.
                let vpy = svp::write_vpy_script(
                    args.session_dir,
                    24,
                    1,
                    args.frame_gen_settings.svp.multiplier,
                )?;
                binary = vs_mpv;
                extra_args.push(format!("--vf=vapoursynth={}", vpy.display()));
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "no vapoursynth-enabled mpv available, falling back to plain playback"
                );
            }
        },
    }

    if args.disable_native_subs {
        extra_args.push("--sid=no".to_string());
    }
    if let Some(seconds) = args.resume_seconds {
        extra_args.push(format!("--start={seconds}"));
    }

    let mut command = Command::new(&binary);
    command
        .arg(format!("--wid={}", args.container_window))
        .arg("--no-config")
        .arg("--no-border")
        .arg("--gpu-context=x11vk")
        .arg("--vo=gpu-next")
        .arg("--gpu-api=vulkan")
        .arg(format!("--input-ipc-server={}", args.socket_path.display()))
        .args(&extra_args)
        .arg(args.stream_url);

    let child = command
        .spawn()
        .with_context(|| format!("failed to spawn mpv ({})", binary.display()))?;
    Ok(MpvLaunch { child })
}
