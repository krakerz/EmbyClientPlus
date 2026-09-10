pub mod mpv_ipc;
pub mod mpv_process;
pub mod window_sync;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::Settings;
use crate::db::TitleOverride;
use crate::emby::EmbyClient;
use crate::emby::models::{DeviceProfile, PlayingRequest, ProgressRequest, StoppedRequest};

/// Resolves the per-series/movie override key for an item: the item's
/// own id for movies, or its parent `SeriesId` for episodes, so a change
/// made on one episode applies to the whole series. A plain async fn
/// (not folded into `start_playback`) so the caller can look the
/// override up synchronously *between* this and `start_playback` —
/// `rusqlite::Connection` isn't `Sync`, so a `Db` reference can't be held
/// across an `.await` inside a Tauri command's future at all.
pub async fn resolve_override_key(
    emby: &EmbyClient,
    user_id: &str,
    item_id: &str,
) -> Result<String> {
    match emby.get_item_series_id(user_id, item_id).await? {
        Some(series_id) => Ok(series_id),
        None => Ok(item_id.to_string()),
    }
}

/// Starts playback for `item_id`: negotiates PlaybackInfo (applying
/// `title_override` over `settings`'s global defaults), spawns mpv
/// embedded in its own X11 container window, and reports the session to
/// Emby. Returns once playback has genuinely started (mpv's IPC socket
/// is connected) — a background task (spawned internally) owns the rest
/// of the session's lifecycle (progress reporting, teardown on exit), so
/// the caller doesn't need to hold onto anything further. Back-
/// navigation-triggered teardown isn't wired in yet (M10, once overlay
/// controls exist) — for now the session only ends when mpv itself
/// exits.
pub async fn start_playback(
    emby: EmbyClient,
    title_override: Option<TitleOverride>,
    settings: &Settings,
    user_id: &str,
    item_id: &str,
) -> Result<()> {
    let backend = crate::frame_gen::resolve_backend(
        settings.frame_gen.default_backend,
        title_override.as_ref().and_then(|o| o.frame_gen_backend),
    );

    let bitrate_cap = if settings.playback.bitrate_cap_mbps > 0 {
        Some(settings.playback.bitrate_cap_mbps as i64 * 1_000_000)
    } else {
        None
    };

    let playback_info = emby
        .get_playback_info(user_id, item_id, DeviceProfile::default(), bitrate_cap, 0)
        .await?;
    let media_source = playback_info
        .media_sources
        .first()
        .context("Emby returned no playable media source for this item")?;
    let media_source_id = media_source.id.clone();

    let stream_url = emby.direct_stream_url(item_id, &media_source_id)?;

    let session_dir = std::env::temp_dir().join(format!("embyclientplus-{item_id}"));
    std::fs::create_dir_all(&session_dir)
        .with_context(|| format!("failed to create {}", session_dir.display()))?;
    let socket_path = session_dir.join("mpv.sock");

    // Placeholder geometry — keeping this in lockstep with the main
    // window is M10's job, once overlay controls exist to justify the
    // extra complexity.
    let container = window_sync::ContainerWindow::create(0, 0, 1280, 720)?;

    let mut launch = match mpv_process::spawn(mpv_process::SpawnArgs {
        backend,
        frame_gen_settings: &settings.frame_gen,
        session_dir: &session_dir,
        container_window: container.window,
        socket_path: &socket_path,
        stream_url: &stream_url,
    }) {
        Ok(launch) => launch,
        Err(e) => {
            let _ = container.destroy();
            return Err(e);
        }
    };

    let mpv = match mpv_ipc::connect_with_retry(&socket_path) {
        Ok(mpv) => mpv,
        Err(e) => {
            let _ = launch.child.kill();
            let _ = container.destroy();
            return Err(e);
        }
    };

    emby.report_playing(&PlayingRequest {
        item_id: item_id.to_string(),
        media_source_id: media_source_id.clone(),
        play_session_id: playback_info.play_session_id.clone(),
        position_ticks: 0,
        is_paused: false,
        can_seek: true,
    })
    .await?;

    tokio::spawn(run_session(
        emby,
        SessionIds {
            item_id: item_id.to_string(),
            media_source_id,
            play_session_id: playback_info.play_session_id,
        },
        container,
        mpv,
        launch.child,
        socket_path,
    ));

    Ok(())
}

struct SessionIds {
    item_id: String,
    media_source_id: String,
    play_session_id: String,
}

/// Owns a playback session once it's underway: reports progress every
/// 10s, and on mpv exiting (for any reason) reports Stopped and tears
/// down the container window + socket file.
async fn run_session(
    emby: EmbyClient,
    ids: SessionIds,
    container: window_sync::ContainerWindow,
    mpv: mpvipc::Mpv,
    mut child: std::process::Child,
    socket_path: PathBuf,
) {
    // Track the last successfully-read position/pause state rather than
    // querying mpv again once we know it has exited — confirmed live
    // that the mpvipc crate *panics* (not a clean `Err`) when writing to
    // an already-broken socket, so this isn't just tidiness, it avoids a
    // real crash.
    let mut last_position_ticks: i64 = 0;
    let mut last_is_paused = false;

    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "failed to poll mpv process status");
                break;
            }
        }

        if let Ok(position_ticks) = mpv_ipc::current_position_ticks(&mpv) {
            last_position_ticks = position_ticks;
        }
        if let Ok(is_paused) = mpv_ipc::is_paused(&mpv) {
            last_is_paused = is_paused;
        }
        if let Err(e) = emby
            .report_progress(&ProgressRequest {
                item_id: ids.item_id.clone(),
                media_source_id: ids.media_source_id.clone(),
                play_session_id: ids.play_session_id.clone(),
                position_ticks: last_position_ticks,
                is_paused: last_is_paused,
                can_seek: true,
                event_name: "timeupdate".to_string(),
            })
            .await
        {
            tracing::warn!(error = %e, "failed to report playback progress");
        }

        tokio::time::sleep(Duration::from_secs(10)).await;
    }

    if let Err(e) = emby
        .report_stopped(&StoppedRequest {
            item_id: ids.item_id,
            media_source_id: ids.media_source_id,
            play_session_id: ids.play_session_id,
            position_ticks: last_position_ticks,
        })
        .await
    {
        tracing::warn!(error = %e, "failed to report playback stopped");
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = container.destroy();
    let _ = std::fs::remove_file(&socket_path);
}
