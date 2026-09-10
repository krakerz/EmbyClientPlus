pub mod bitmap_font;
pub mod mpv_ipc;
pub mod mpv_process;
pub mod overlay_window;
pub mod window_sync;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tauri::WebviewWindow;

use crate::config::{FrameGenBackend, FrameGenSettings, Settings};
use crate::db::{Db, ItemType, TitleOverride};
use crate::emby::EmbyClient;
use crate::emby::models::{
    ChapterInfo, DeviceProfile, ItemSummary, MediaStream, PlayingRequest, ProgressRequest,
    StoppedRequest,
};
use crate::subtitles::SubtitleRenderer;

const CONTAINER_WIDTH: u16 = 1280;
const CONTAINER_HEIGHT: u16 = 720;
const OVERLAY_TICK: Duration = Duration::from_millis(200);
const PROGRESS_REPORT_INTERVAL: Duration = Duration::from_secs(10);

const BUTTON_WIDTH: i32 = 140;
const BUTTON_HEIGHT: i32 = 36;
const BUTTON_GAP: i32 = 8;
const BUTTON_MARGIN: i32 = 12;
const BUTTON_COLOR: (u8, u8, u8, u8) = (40, 44, 52, 200);
const LABEL_COLOR: (u8, u8, u8, u8) = (235, 235, 235, 255);
const LABEL_SCALE: i32 = 2;

/// How long the seek bar / chapter ticks / title+time text (together, the
/// "OSD") stay visible after the last click or pointer motion, before
/// fading out. The back button, prev/next-episode buttons, and the 4
/// quick-toggle buttons are deliberately *not* part of this — they stay
/// always-visible, unchanged from how M10 shipped them.
const OSD_VISIBLE_DURATION: Duration = Duration::from_secs(3);
const OSD_MARGIN: i32 = 20;
const OSD_ROW_GAP: i32 = 8;
const OSD_TITLE_MAX_CHARS: usize = 60;
const SEEKBAR_HEIGHT: i32 = 8;
const SEEKBAR_BG_COLOR: (u8, u8, u8, u8) = (255, 255, 255, 60);
const SEEKBAR_FILL_COLOR: (u8, u8, u8, u8) = (235, 235, 235, 220);
const CHAPTER_TICK_COLOR: (u8, u8, u8, u8) = (255, 200, 60, 230);
const CHAPTER_TICK_WIDTH: i32 = 2;
/// Clicking within this many pixels of a chapter tick snaps to that
/// chapter's exact start instead of the raw click fraction.
const CHAPTER_SNAP_TOLERANCE_PX: i32 = 8;

/// Bitrate caps offered by the quality quick-toggle, as `(label,
/// bitrate_cap_mbps)` — `0` means "Auto" (today's default: always
/// direct-stream the original file, no cap requested at all).
const QUALITY_PRESETS: [(&str, u32); 4] = [("AUTO", 0), ("20MB", 20), ("10MB", 10), ("4MB", 4)];

const FRAME_GEN_BACKENDS: [FrameGenBackend; 3] = [
    FrameGenBackend::Off,
    FrameGenBackend::Svp,
    FrameGenBackend::LsfgVk,
];

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

/// Text-based formats libass can read directly (SRT/ASS/SSA/VTT).
/// Image-based subtitle tracks (PGS/VobSub) aren't rendered on our own
/// layer for v1 — see project CLAUDE.md's graceful-fallback pattern;
/// they're simply not selectable here, so mpv's own native subtitle
/// rendering stays in play for that track (ghosting under lsfg-vk is a
/// known, accepted limitation for that specific combination, not this
/// codec-selection logic's job to solve).
fn subtitle_format_extension(codec: &str) -> Option<&'static str> {
    match codec.to_ascii_lowercase().as_str() {
        "ass" => Some("ass"),
        "ssa" => Some("ssa"),
        "subrip" | "srt" => Some("srt"),
        "webvtt" | "vtt" => Some("vtt"),
        _ => None,
    }
}

fn select_text_subtitle_streams(
    media_streams: &[MediaStream],
) -> Vec<(&MediaStream, &'static str)> {
    media_streams
        .iter()
        .filter_map(|stream| {
            if stream.stream_type != "Subtitle" {
                return None;
            }
            let codec = stream.codec.as_deref()?;
            let ext = subtitle_format_extension(codec)?;
            Some((stream, ext))
        })
        .collect()
}

/// One selectable text-based subtitle track — just enough owned data to
/// survive into the long-lived `run_session` task (no borrow of
/// `MediaStream`/the PlaybackInfo response it came from).
struct SubtitleOption {
    stream_index: i32,
    ext: &'static str,
    language_label: String,
}

fn build_subtitle_options(media_streams: &[MediaStream]) -> Vec<SubtitleOption> {
    select_text_subtitle_streams(media_streams)
        .into_iter()
        .map(|(stream, ext)| SubtitleOption {
            stream_index: stream.index,
            ext,
            language_label: language_label(stream.language.as_deref()),
        })
        .collect()
}

/// Normalizes a raw language string (Emby's `MediaStream.language`, or
/// mpv's own `track-list/N/lang`) into the short uppercase form used both
/// for button labels and for matching against a saved override — this is
/// the single source of truth both sides compare against.
fn language_label(language: Option<&str>) -> String {
    language
        .unwrap_or("und")
        .chars()
        .take(3)
        .collect::<String>()
        .to_uppercase()
}

fn backend_label(backend: FrameGenBackend) -> &'static str {
    match backend {
        FrameGenBackend::Off => "OFF",
        FrameGenBackend::Svp => "SVP",
        FrameGenBackend::LsfgVk => "LSFG",
    }
}

fn initial_subtitle_selection(
    options: &[SubtitleOption],
    preferred_language: Option<&str>,
) -> Option<usize> {
    if let Some(lang) = preferred_language
        && let Some(i) = options
            .iter()
            .position(|o| o.language_label.eq_ignore_ascii_case(lang))
    {
        return Some(i);
    }
    if options.is_empty() { None } else { Some(0) }
}

fn initial_audio_selection(tracks: &[(i64, String)], preferred_language: Option<&str>) -> usize {
    if let Some(lang) = preferred_language
        && let Some(i) = tracks
            .iter()
            .position(|(_, track_lang)| language_label(Some(track_lang)).eq_ignore_ascii_case(lang))
    {
        return i;
    }
    0
}

/// Runtime state for the 4 bottom-right quick-toggle buttons. Audio and
/// subtitle switching happen without restarting mpv (mpv itself demuxes
/// every embedded audio track; subtitle rendering is our own independent
/// libass pipeline); quality and frame-gen changes both restart the mpv
/// child in place (see `restart_mpv_at`), since both are otherwise only
/// decided at spawn time.
struct QuickToggles {
    subtitle_options: Vec<SubtitleOption>,
    subtitle_selected: Option<usize>,
    audio_tracks: Vec<(i64, String)>,
    audio_selected: usize,
    quality_selected: usize,
    frame_gen_selected: usize,
}

impl QuickToggles {
    fn subtitle_label(&self) -> String {
        match self.subtitle_selected {
            _ if self.subtitle_options.is_empty() => "C:N/A".to_string(),
            None => "C:OFF".to_string(),
            Some(i) => format!("C:{}", self.subtitle_options[i].language_label),
        }
    }

    fn audio_label(&self) -> String {
        match self.audio_tracks.get(self.audio_selected) {
            Some((_, lang)) => format!("A:{}", language_label(Some(lang))),
            None => "A:N/A".to_string(),
        }
    }

    fn quality_label(&self) -> String {
        format!("Q:{}", QUALITY_PRESETS[self.quality_selected].0)
    }

    fn frame_gen_label(&self) -> String {
        format!(
            "F:{}",
            backend_label(FRAME_GEN_BACKENDS[self.frame_gen_selected])
        )
    }

    /// Cycles Off → track 0 → track 1 → ... → Off.
    fn cycle_subtitle(&mut self) {
        if self.subtitle_options.is_empty() {
            return;
        }
        self.subtitle_selected = match self.subtitle_selected {
            None => Some(0),
            Some(i) if i + 1 < self.subtitle_options.len() => Some(i + 1),
            Some(_) => None,
        };
    }

    fn cycle_audio(&mut self) {
        if self.audio_tracks.is_empty() {
            return;
        }
        self.audio_selected = (self.audio_selected + 1) % self.audio_tracks.len();
    }

    fn cycle_quality(&mut self) {
        self.quality_selected = (self.quality_selected + 1) % QUALITY_PRESETS.len();
    }

    fn cycle_frame_gen(&mut self) {
        self.frame_gen_selected = (self.frame_gen_selected + 1) % FRAME_GEN_BACKENDS.len();
    }
}

/// Title/chapters/prev-next-episode data for the currently-playing item —
/// resolved once per `load_item` call via `resolve_playback_metadata`.
struct PlaybackMetadata {
    title: String,
    /// Chapter start times in seconds (Emby's ticks already converted) —
    /// just enough to draw seek-bar tick marks and snap clicks to them;
    /// chapter names aren't currently displayed anywhere.
    chapters: Vec<f64>,
    prev_item_id: Option<String>,
    next_item_id: Option<String>,
}

fn chapter_seconds(chapters: &[ChapterInfo]) -> Vec<f64> {
    chapters
        .iter()
        .map(|c| c.start_position_ticks as f64 / 10_000_000.0)
        .collect()
}

fn format_item_title(item: &ItemSummary) -> String {
    let mut title = match (item.parent_index_number, item.index_number) {
        (Some(season), Some(episode)) => match &item.series_name {
            Some(series) => format!("{series} S{season}E{episode} {}", item.name),
            None => format!("S{season}E{episode} {}", item.name),
        },
        _ => item.name.clone(),
    };
    if title.chars().count() > OSD_TITLE_MAX_CHARS {
        title = title.chars().take(OSD_TITLE_MAX_CHARS).collect();
        title.push_str("...");
    }
    title
}

/// Resolves the current item's OSD title, chapter marks, and previous/
/// next-episode neighbors. For a series episode, this is a single call —
/// the full sibling episode list (already carrying each entry's own
/// `Chapters`) both locates the neighbors *and* supplies the current
/// item's own title/chapters, via whichever entry matches `item_id`. For
/// a movie (no series), falls back to a direct single-item lookup, and
/// prev/next both stay `None`.
async fn resolve_playback_metadata(
    emby: &EmbyClient,
    user_id: &str,
    item_id: &str,
    item_type: ItemType,
    series_id: &str,
) -> Result<PlaybackMetadata> {
    if item_type == ItemType::Series {
        let episodes = emby.get_series_episodes(series_id, user_id).await?;
        if let Some(idx) = episodes.iter().position(|e| e.id == item_id) {
            return Ok(PlaybackMetadata {
                title: format_item_title(&episodes[idx]),
                chapters: chapter_seconds(&episodes[idx].chapters),
                prev_item_id: idx.checked_sub(1).map(|i| episodes[i].id.clone()),
                next_item_id: episodes.get(idx + 1).map(|e| e.id.clone()),
            });
        }
        // The current item wasn't in its own series' episode list (not
        // expected in practice) — fall through to the movie-style single
        // lookup below rather than failing playback metadata entirely.
    }
    let item = emby.get_item_summary(user_id, item_id).await?;
    Ok(PlaybackMetadata {
        title: format_item_title(&item),
        chapters: chapter_seconds(&item.chapters),
        prev_item_id: None,
        next_item_id: None,
    })
}

/// Draws the "back to library" click target into an already-allocated
/// RGBA canvas — a solid, semi-opaque rectangle for now; real
/// iconography/labeling is future visual polish, not this milestone's
/// job. Must match [`overlay_window::BACK_BUTTON`]'s bounds, which is
/// what click detection actually checks against.
fn draw_back_button(canvas: &mut [u8], canvas_width: u32, canvas_height: u32) {
    let (bx, by, bw, bh) = overlay_window::BACK_BUTTON;
    fill_rect(
        canvas,
        canvas_width,
        canvas_height,
        (bx as i32, by as i32, bw as i32, bh as i32),
        BUTTON_COLOR,
    );
}

/// Draws one quick-toggle (or nav) button: a solid background rect plus
/// its label, via the hand-rolled bitmap font (see `bitmap_font` — not
/// libass, which stays scoped to real subtitle tracks).
fn draw_button(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    rect: (i32, i32, i32, i32),
    label: &str,
) {
    fill_rect(canvas, canvas_width, canvas_height, rect, BUTTON_COLOR);
    let (bx, by, bw, bh) = rect;
    let text_y = by + (bh - bitmap_font::text_height(LABEL_SCALE)) / 2;
    let text_x = bx + (bw - bitmap_font::text_width(label, LABEL_SCALE)).max(0) / 2;
    bitmap_font::draw_text(
        canvas,
        canvas_width,
        canvas_height,
        text_x,
        text_y,
        label,
        LABEL_SCALE,
        LABEL_COLOR,
    );
}

fn fill_rect(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    rect: (i32, i32, i32, i32),
    color: (u8, u8, u8, u8),
) {
    let (rx, ry, rw, rh) = rect;
    let (r, g, b, a) = color;
    for y in ry.max(0)..(ry + rh) {
        if y as u32 >= canvas_height {
            break;
        }
        for x in rx.max(0)..(rx + rw) {
            if x as u32 >= canvas_width {
                break;
            }
            let offset = ((y as u32 * canvas_width + x as u32) * 4) as usize;
            canvas[offset] = r;
            canvas[offset + 1] = g;
            canvas[offset + 2] = b;
            canvas[offset + 3] = a;
        }
    }
}

/// The 4 quick-toggle buttons' rects (x, y, w, h), bottom-right anchored
/// so they stay reachable regardless of the current window size. Order
/// matches [`QuickToggles`]'s label methods: subtitles, audio, quality,
/// frame-gen.
fn quick_toggle_rects(canvas_width: u32, canvas_height: u32) -> [(i32, i32, i32, i32); 4] {
    let total_width = 4 * BUTTON_WIDTH + 3 * BUTTON_GAP;
    let start_x = canvas_width as i32 - BUTTON_MARGIN - total_width;
    let y = canvas_height as i32 - BUTTON_MARGIN - BUTTON_HEIGHT;
    std::array::from_fn(|i| {
        let x = start_x + i as i32 * (BUTTON_WIDTH + BUTTON_GAP);
        (x, y, BUTTON_WIDTH, BUTTON_HEIGHT)
    })
}

/// The seek bar's rect, positioned directly above the quick-toggle row.
fn seek_bar_rect(canvas_width: u32, canvas_height: u32) -> (i32, i32, i32, i32) {
    let toggle_y = canvas_height as i32 - BUTTON_MARGIN - BUTTON_HEIGHT;
    let y = toggle_y - OSD_ROW_GAP - SEEKBAR_HEIGHT;
    (
        OSD_MARGIN,
        y,
        canvas_width as i32 - 2 * OSD_MARGIN,
        SEEKBAR_HEIGHT,
    )
}

/// The title/time text row's baseline y, directly above the seek bar.
fn osd_text_y(canvas_width: u32, canvas_height: u32) -> i32 {
    let (_, seek_y, _, _) = seek_bar_rect(canvas_width, canvas_height);
    seek_y - OSD_ROW_GAP - bitmap_font::text_height(LABEL_SCALE)
}

fn hit_test(rect: (i32, i32, i32, i32), x: i16, y: i16) -> bool {
    let (rx, ry, rw, rh) = rect;
    let (x, y) = (x as i32, y as i32);
    x >= rx && x < rx + rw && y >= ry && y < ry + rh
}

fn is_back_button_click(x: i16, y: i16) -> bool {
    let (bx, by, bw, bh) = overlay_window::BACK_BUTTON;
    hit_test((bx as i32, by as i32, bw as i32, bh as i32), x, y)
}

fn is_prev_button_click(x: i16, y: i16) -> bool {
    let (bx, by, bw, bh) = overlay_window::PREV_BUTTON;
    hit_test((bx as i32, by as i32, bw as i32, bh as i32), x, y)
}

fn is_next_button_click(x: i16, y: i16) -> bool {
    let (bx, by, bw, bh) = overlay_window::NEXT_BUTTON;
    hit_test((bx as i32, by as i32, bw as i32, bh as i32), x, y)
}

/// Draws the seek bar's fill + chapter tick marks, and the title/time
/// text row above it. Only called while the OSD is visible.
fn draw_osd(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    title: &str,
    position_seconds: f64,
    duration_seconds: f64,
    chapters: &[f64],
) {
    let rect @ (bx, by, bw, bh) = seek_bar_rect(canvas_width, canvas_height);
    fill_rect(canvas, canvas_width, canvas_height, rect, SEEKBAR_BG_COLOR);
    if duration_seconds > 0.0 {
        let fraction = (position_seconds / duration_seconds).clamp(0.0, 1.0);
        let fill_width = (bw as f64 * fraction) as i32;
        fill_rect(
            canvas,
            canvas_width,
            canvas_height,
            (bx, by, fill_width, bh),
            SEEKBAR_FILL_COLOR,
        );
        for &chapter_seconds in chapters {
            let chapter_fraction = (chapter_seconds / duration_seconds).clamp(0.0, 1.0);
            let tick_x = bx + (bw as f64 * chapter_fraction) as i32;
            fill_rect(
                canvas,
                canvas_width,
                canvas_height,
                (tick_x, by, CHAPTER_TICK_WIDTH, bh),
                CHAPTER_TICK_COLOR,
            );
        }
    }

    let text_y = osd_text_y(canvas_width, canvas_height);
    bitmap_font::draw_text(
        canvas,
        canvas_width,
        canvas_height,
        OSD_MARGIN,
        text_y,
        title,
        LABEL_SCALE,
        LABEL_COLOR,
    );
    let time_label = format!(
        "{}/{}",
        format_time(position_seconds),
        format_time(duration_seconds)
    );
    let time_x =
        canvas_width as i32 - OSD_MARGIN - bitmap_font::text_width(&time_label, LABEL_SCALE);
    bitmap_font::draw_text(
        canvas,
        canvas_width,
        canvas_height,
        time_x,
        text_y,
        &time_label,
        LABEL_SCALE,
        LABEL_COLOR,
    );
}

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as i64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// Resolves a seek-bar click's x coordinate into a target mpv position in
/// seconds, snapping to the nearest chapter if the click landed within
/// [`CHAPTER_SNAP_TOLERANCE_PX`] of one.
fn seek_bar_click_seconds(
    rect: (i32, i32, i32, i32),
    duration_seconds: f64,
    chapters: &[f64],
    click_x: i16,
) -> f64 {
    let (rx, _, rw, _) = rect;
    let fraction = ((click_x as i32 - rx) as f64 / rw as f64).clamp(0.0, 1.0);
    let raw_seconds = fraction * duration_seconds;
    let tolerance_seconds = (CHAPTER_SNAP_TOLERANCE_PX as f64 / rw as f64) * duration_seconds;
    chapters
        .iter()
        .copied()
        .min_by(|a, b| {
            (a - raw_seconds)
                .abs()
                .partial_cmp(&(b - raw_seconds).abs())
                .unwrap()
        })
        .filter(|nearest| (nearest - raw_seconds).abs() <= tolerance_seconds)
        .unwrap_or(raw_seconds)
}

/// Re-negotiates PlaybackInfo for a quality quick-toggle change and
/// resolves the resulting stream URL: "Auto" (`bitrate_cap_mbps == 0`)
/// always keeps today's direct-stream behavior; any capped preset prefers
/// Emby's `TranscodingUrl` when the server actually returned one (i.e.
/// decided the cap requires transcoding), falling back to direct-stream
/// if the source already fits under the cap without one.
async fn negotiate_stream_url(
    emby: &EmbyClient,
    user_id: &str,
    item_id: &str,
    bitrate_cap_mbps: u32,
    resume_ticks: i64,
) -> Result<(String, String)> {
    let bitrate_cap = if bitrate_cap_mbps > 0 {
        Some(bitrate_cap_mbps as i64 * 1_000_000)
    } else {
        None
    };
    let info = emby
        .get_playback_info(
            user_id,
            item_id,
            DeviceProfile::default(),
            bitrate_cap,
            resume_ticks,
        )
        .await?;
    let media_source = info
        .media_sources
        .first()
        .context("Emby returned no playable media source for this item")?;
    let stream_url = if bitrate_cap_mbps == 0 {
        emby.direct_stream_url(item_id, &media_source.id)?
    } else if let Some(transcoding_url) = &media_source.transcoding_url {
        emby.resolve_transcoding_url(transcoding_url)
    } else {
        emby.direct_stream_url(item_id, &media_source.id)?
    };
    Ok((stream_url, media_source.id.clone()))
}

/// Restarts the mpv child in place for a quality or frame-gen quick-
/// toggle change: kills the old process, clears its socket file, spawns
/// a fresh one with `--start=` set to resume near `resume_ticks`,
/// reconnects IPC, and best-effort reapplies the current audio track
/// selection (mpv's own track ids don't necessarily carry over from
/// session state otherwise). The subtitle renderer is untouched by any
/// of this — it's a fully independent pipeline driven by `time-pos`
/// reads, unaffected by mpv restarting underneath it.
#[allow(clippy::too_many_arguments)]
fn restart_mpv_at(
    child: &mut std::process::Child,
    mpv: &mut mpvipc::Mpv,
    socket_path: &Path,
    container_window: u32,
    frame_gen_settings: &FrameGenSettings,
    backend: FrameGenBackend,
    session_dir: &Path,
    stream_url: &str,
    disable_native_subs: bool,
    resume_ticks: i64,
    reapply_audio: Option<i64>,
) -> Result<()> {
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(socket_path);

    let launch = mpv_process::spawn(mpv_process::SpawnArgs {
        backend,
        frame_gen_settings,
        session_dir,
        container_window,
        socket_path,
        stream_url,
        disable_native_subs,
        resume_seconds: Some(resume_ticks as f64 / 10_000_000.0),
    })?;
    *child = launch.child;
    *mpv = mpv_ipc::connect_with_retry(socket_path)?;
    if let Some(aid) = reapply_audio
        && let Err(e) = mpv_ipc::set_audio_track(mpv, aid)
    {
        tracing::warn!(error = %e, "failed to reapply audio track after mpv restart");
    }
    Ok(())
}

/// Persists a change to the per-series/movie override, merging into
/// whatever's already saved (so e.g. an audio-language change doesn't
/// clobber a previously-saved subtitle preference). Best-effort — a
/// failure here shouldn't interrupt playback, just logs a warning.
fn write_override(
    override_key: &str,
    item_type: ItemType,
    mutate: impl FnOnce(&mut TitleOverride),
) {
    let db = match Db::open_default() {
        Ok(db) => db,
        Err(e) => {
            tracing::warn!(error = %e, "failed to open overrides db");
            return;
        }
    };
    let mut entry = db
        .get_override(override_key)
        .ok()
        .flatten()
        .unwrap_or(TitleOverride {
            emby_item_id: override_key.to_string(),
            item_type,
            audio_language: None,
            subtitle_language: None,
            subtitle_forced_only: None,
            frame_gen_backend: None,
            frame_gen_multiplier: None,
        });
    mutate(&mut entry);
    if let Err(e) = db.upsert_override(&entry) {
        tracing::warn!(error = %e, "failed to persist title override");
    }
}

/// Reads whatever override is currently saved for `override_key`,
/// synchronously (never held across an `.await` — see `write_override`'s
/// doc comment for why). Best-effort: a read failure just means playback
/// falls back to global defaults, same as no override existing at all.
fn read_override(override_key: &str) -> Option<TitleOverride> {
    Db::open_default()
        .ok()?
        .get_override(override_key)
        .ok()
        .flatten()
}

/// Config that stays constant across the whole window session — reused
/// by both `load_item` (spawning mpv for a newly-loaded item) and
/// `restart_mpv_at` (respawning the *same* item under a new quality/
/// frame-gen quick-toggle choice).
struct PlaybackConfig {
    session_dir: PathBuf,
    socket_path: PathBuf,
    frame_gen_settings: FrameGenSettings,
    default_frame_gen_backend: FrameGenBackend,
    bitrate_cap_mbps: u32,
}

/// Everything `load_item` produces for a freshly (re)loaded item.
struct LoadedItem {
    media_source_id: String,
    play_session_id: String,
    stream_url: String,
    subtitle_renderer: Option<SubtitleRenderer>,
    toggles: QuickToggles,
    mpv: mpvipc::Mpv,
    child: std::process::Child,
    metadata: PlaybackMetadata,
}

/// Negotiates PlaybackInfo, builds the initial subtitle/audio/quality/
/// frame-gen quick-toggle state (applying `title_override` the same way
/// `start_playback` always has), spawns mpv into `container_window`, and
/// resolves this item's title/chapters/prev-next-episode metadata.
/// Extracted out of `start_playback` so a next/previous-episode click can
/// call it again for a different `item_id` against the *same* container/
/// overlay windows, instead of tearing them down and recreating them
/// (which would flash the video briefly for no reason).
#[allow(clippy::too_many_arguments)]
async fn load_item(
    emby: &EmbyClient,
    config: &PlaybackConfig,
    title_override: Option<&TitleOverride>,
    user_id: &str,
    item_id: &str,
    item_type: ItemType,
    override_key: &str,
    container_window: u32,
    canvas_width: u32,
    canvas_height: u32,
) -> Result<LoadedItem> {
    let backend = crate::frame_gen::resolve_backend(
        config.default_frame_gen_backend,
        title_override.and_then(|o| o.frame_gen_backend),
    );
    let bitrate_cap = if config.bitrate_cap_mbps > 0 {
        Some(config.bitrate_cap_mbps as i64 * 1_000_000)
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

    let subtitle_options = build_subtitle_options(&media_source.media_streams);
    let subtitle_selected = initial_subtitle_selection(
        &subtitle_options,
        title_override.and_then(|o| o.subtitle_language.as_deref()),
    );
    // Unconditional whenever any text-based track exists, regardless of
    // the initial selection — our own overlay is the sole subtitle
    // authority from that point on, so mpv's native rendering must stay
    // out of the way even if the user starts with subtitles off.
    let disable_native_subs = !subtitle_options.is_empty();

    let subtitle_renderer = match subtitle_selected {
        Some(i) => {
            let opt = &subtitle_options[i];
            match crate::subtitles::fetch_subtitle_content(
                emby,
                item_id,
                &media_source_id,
                opt.stream_index,
                opt.ext,
            )
            .await
            {
                Ok(content) => match SubtitleRenderer::new(canvas_width, canvas_height, &content) {
                    Ok(renderer) => Some(renderer),
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to initialize subtitle renderer");
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "failed to fetch subtitle content");
                    None
                }
            }
        }
        None => None,
    };

    let mut launch = mpv_process::spawn(mpv_process::SpawnArgs {
        backend,
        frame_gen_settings: &config.frame_gen_settings,
        session_dir: &config.session_dir,
        container_window,
        socket_path: &config.socket_path,
        stream_url: &stream_url,
        disable_native_subs,
        resume_seconds: None,
    })?;

    let mpv = match mpv_ipc::connect_with_retry(&config.socket_path) {
        Ok(mpv) => mpv,
        Err(e) => {
            let _ = launch.child.kill();
            return Err(e);
        }
    };

    let audio_tracks = mpv_ipc::audio_tracks_with_retry(&mpv).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to read mpv audio tracks");
        Vec::new()
    });
    let audio_selected = initial_audio_selection(
        &audio_tracks,
        title_override.and_then(|o| o.audio_language.as_deref()),
    );
    if audio_selected != 0
        && let Some((id, _)) = audio_tracks.get(audio_selected)
        && let Err(e) = mpv_ipc::set_audio_track(&mpv, *id)
    {
        tracing::warn!(error = %e, "failed to apply preferred audio track");
    }

    let frame_gen_selected = FRAME_GEN_BACKENDS
        .iter()
        .position(|b| *b == backend)
        .unwrap_or(0);
    let toggles = QuickToggles {
        subtitle_options,
        subtitle_selected,
        audio_tracks,
        audio_selected,
        quality_selected: 0,
        frame_gen_selected,
    };

    let metadata = resolve_playback_metadata(emby, user_id, item_id, item_type, override_key)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to resolve playback metadata (title/chapters/prev-next)");
            PlaybackMetadata {
                title: String::new(),
                chapters: Vec::new(),
                prev_item_id: None,
                next_item_id: None,
            }
        });

    Ok(LoadedItem {
        media_source_id,
        play_session_id: playback_info.play_session_id,
        stream_url,
        subtitle_renderer,
        toggles,
        mpv,
        child: launch.child,
        metadata,
    })
}

/// Starts playback for `item_id`: creates the container/overlay X11
/// windows, loads the item into them via `load_item`, and reports the
/// session to Emby. Returns once playback has genuinely started (mpv's
/// IPC socket is connected) — a background task (spawned internally) owns
/// the rest of the session's lifecycle (subtitle refresh, geometry
/// lockstep with `main_window`, quick-toggle/back/prev/next-episode
/// handling, progress reporting, teardown on exit), so the caller doesn't
/// need to hold onto anything further.
#[allow(clippy::too_many_arguments)]
pub async fn start_playback(
    emby: EmbyClient,
    title_override: Option<TitleOverride>,
    settings: &Settings,
    user_id: &str,
    item_id: &str,
    main_window: WebviewWindow,
    override_key: String,
    item_type: ItemType,
) -> Result<()> {
    let session_dir = std::env::temp_dir().join(format!("embyclientplus-{item_id}"));
    std::fs::create_dir_all(&session_dir)
        .with_context(|| format!("failed to create {}", session_dir.display()))?;
    let socket_path = session_dir.join("mpv.sock");

    // Order matters here: X11 stacks newly created windows on top of
    // existing siblings, so the container (mpv's video) must be created
    // *before* the overlay — reversing this silently hides the overlay's
    // content underneath the opaque video layer (confirmed live during
    // M9 — see NOTES.md).
    let container = window_sync::ContainerWindow::create(0, 0, CONTAINER_WIDTH, CONTAINER_HEIGHT)?;
    let overlay =
        match overlay_window::OverlayWindow::create(0, 0, CONTAINER_WIDTH, CONTAINER_HEIGHT) {
            Ok(overlay) => overlay,
            Err(e) => {
                let _ = container.destroy();
                return Err(e.context("failed to create overlay window"));
            }
        };

    let config = PlaybackConfig {
        session_dir,
        socket_path,
        frame_gen_settings: settings.frame_gen.clone(),
        default_frame_gen_backend: settings.frame_gen.default_backend,
        bitrate_cap_mbps: settings.playback.bitrate_cap_mbps,
    };

    let loaded = match load_item(
        &emby,
        &config,
        title_override.as_ref(),
        user_id,
        item_id,
        item_type,
        &override_key,
        container.window,
        CONTAINER_WIDTH as u32,
        CONTAINER_HEIGHT as u32,
    )
    .await
    {
        Ok(loaded) => loaded,
        Err(e) => {
            let _ = overlay.destroy();
            let _ = container.destroy();
            return Err(e);
        }
    };

    emby.report_playing(&PlayingRequest {
        item_id: item_id.to_string(),
        media_source_id: loaded.media_source_id.clone(),
        play_session_id: loaded.play_session_id.clone(),
        position_ticks: 0,
        is_paused: false,
        can_seek: true,
    })
    .await?;

    let session_ids = SessionIds {
        user_id: user_id.to_string(),
        item_id: item_id.to_string(),
        media_source_id: loaded.media_source_id,
        play_session_id: loaded.play_session_id,
        override_key,
        item_type,
    };

    tokio::spawn(run_session(
        emby,
        session_ids,
        config,
        container,
        overlay,
        loaded.subtitle_renderer,
        loaded.toggles,
        loaded.metadata,
        loaded.mpv,
        loaded.child,
        loaded.stream_url,
        main_window,
    ));

    Ok(())
}

struct SessionIds {
    user_id: String,
    item_id: String,
    media_source_id: String,
    play_session_id: String,
    override_key: String,
    item_type: ItemType,
}

/// Why the current item's tick loop ended.
enum SessionExit {
    /// Back button clicked, or mpv's process exited on its own.
    Stopped,
    /// Previous/next-episode button clicked, carrying the target item id.
    SwitchItem(String),
}

/// Owns a playback session once it's underway. Structured as an outer
/// loop over *items* (initially just the one `start_playback` loaded,
/// growing by one every time a previous/next-episode click succeeds) each
/// wrapping the familiar per-tick inner loop: keeps the container/overlay
/// windows in lockstep with `main_window`, refreshes the overlay
/// (subtitles + OSD + quick-toggle/nav buttons) every [`OVERLAY_TICK`],
/// handles clicks, and reports progress every [`PROGRESS_REPORT_INTERVAL`].
/// A next/previous switch reuses the same container/overlay windows and
/// `PlaybackConfig` (no window recreation, no flash) via `load_item`; a
/// real stop (back button or mpv exiting) tears everything down and
/// navigates `main_window` back to wherever the user came from.
#[allow(clippy::too_many_arguments)]
async fn run_session(
    emby: EmbyClient,
    mut ids: SessionIds,
    config: PlaybackConfig,
    container: window_sync::ContainerWindow,
    mut overlay: overlay_window::OverlayWindow,
    mut subtitle_renderer: Option<SubtitleRenderer>,
    mut toggles: QuickToggles,
    mut metadata: PlaybackMetadata,
    mut mpv: mpvipc::Mpv,
    mut child: std::process::Child,
    mut stream_url: String,
    main_window: WebviewWindow,
) {
    let mut last_geometry: Option<(i32, i32, u32, u32)> = None;
    let mut last_canvas_size = (CONTAINER_WIDTH as u32, CONTAINER_HEIGHT as u32);

    'sessions: loop {
        // Track the last successfully-read position/pause state rather
        // than querying mpv again once we know it has exited — confirmed
        // live that the mpvipc crate *panics* (not a clean `Err`) when
        // writing to an already-broken socket, so this isn't just
        // tidiness, it avoids a real crash.
        let mut last_position_ticks: i64 = 0;
        let mut last_duration_seconds: f64 = 0.0;
        let mut last_is_paused = false;
        let mut since_last_progress_report = PROGRESS_REPORT_INTERVAL; // report immediately on first tick
        let mut osd_deadline = Instant::now() + OSD_VISIBLE_DURATION;
        let exit: SessionExit;

        'ticks: loop {
            match child.try_wait() {
                Ok(Some(_status)) => {
                    exit = SessionExit::Stopped;
                    break 'ticks;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "failed to poll mpv process status");
                    exit = SessionExit::Stopped;
                    break 'ticks;
                }
            }

            if let (Ok(pos), Ok(size)) = (main_window.inner_position(), main_window.inner_size()) {
                let geometry = (pos.x, pos.y, size.width, size.height);
                if last_geometry != Some(geometry) {
                    let (x, y, width, height) = geometry;
                    if let Err(e) = container.reconfigure(x, y, width, height) {
                        tracing::warn!(error = %e, "failed to reposition container window");
                    }
                    if let Err(e) = overlay.reconfigure(x, y, width, height) {
                        tracing::warn!(error = %e, "failed to reposition overlay window");
                    } else if let Some(renderer) = &mut subtitle_renderer {
                        renderer.resize(width, height);
                    }
                    last_geometry = Some(geometry);
                    last_canvas_size = (width, height);
                }
            }
            let (canvas_width, canvas_height) = last_canvas_size;

            let mut switch_target: Option<String> = None;
            match overlay.poll_events() {
                Ok(polled) => {
                    if polled.motion {
                        osd_deadline = Instant::now() + OSD_VISIBLE_DURATION;
                    }
                    let toggle_rects = quick_toggle_rects(canvas_width, canvas_height);
                    let seek_rect = seek_bar_rect(canvas_width, canvas_height);
                    for (cx, cy) in polled.clicks {
                        let osd_visible = Instant::now() < osd_deadline;
                        osd_deadline = Instant::now() + OSD_VISIBLE_DURATION;

                        if is_back_button_click(cx, cy) {
                            tracing::info!("back button clicked, ending playback");
                            exit = SessionExit::Stopped;
                            break 'ticks;
                        } else if metadata.prev_item_id.is_some() && is_prev_button_click(cx, cy) {
                            tracing::info!(item_id = %metadata.prev_item_id.as_deref().unwrap_or(""), "previous episode clicked");
                            switch_target = metadata.prev_item_id.clone();
                        } else if metadata.next_item_id.is_some() && is_next_button_click(cx, cy) {
                            tracing::info!(item_id = %metadata.next_item_id.as_deref().unwrap_or(""), "next episode clicked");
                            switch_target = metadata.next_item_id.clone();
                        } else if osd_visible && hit_test(seek_rect, cx, cy) {
                            let target_seconds = seek_bar_click_seconds(
                                seek_rect,
                                last_duration_seconds,
                                &metadata.chapters,
                                cx,
                            );
                            if let Err(e) = mpv_ipc::seek_to(&mpv, target_seconds) {
                                tracing::warn!(error = %e, "failed to seek mpv");
                            }
                        } else if hit_test(toggle_rects[0], cx, cy) {
                            toggles.cycle_subtitle();
                            subtitle_renderer = match toggles.subtitle_selected {
                                Some(i) => {
                                    let opt = &toggles.subtitle_options[i];
                                    match crate::subtitles::fetch_subtitle_content(
                                        &emby,
                                        &ids.item_id,
                                        &ids.media_source_id,
                                        opt.stream_index,
                                        opt.ext,
                                    )
                                    .await
                                    {
                                        Ok(content) => match SubtitleRenderer::new(
                                            canvas_width,
                                            canvas_height,
                                            &content,
                                        ) {
                                            Ok(r) => Some(r),
                                            Err(e) => {
                                                tracing::warn!(error = %e, "failed to build subtitle renderer");
                                                None
                                            }
                                        },
                                        Err(e) => {
                                            tracing::warn!(error = %e, "failed to fetch subtitle content");
                                            None
                                        }
                                    }
                                }
                                None => None,
                            };
                            let new_language = toggles
                                .subtitle_selected
                                .map(|i| toggles.subtitle_options[i].language_label.clone());
                            write_override(&ids.override_key, ids.item_type, |entry| {
                                entry.subtitle_language = new_language;
                            });
                        } else if hit_test(toggle_rects[1], cx, cy) {
                            toggles.cycle_audio();
                            if let Some((id, lang)) =
                                toggles.audio_tracks.get(toggles.audio_selected).cloned()
                            {
                                if let Err(e) = mpv_ipc::set_audio_track(&mpv, id) {
                                    tracing::warn!(error = %e, "failed to switch mpv audio track");
                                }
                                let normalized = language_label(Some(&lang));
                                write_override(&ids.override_key, ids.item_type, |entry| {
                                    entry.audio_language = Some(normalized);
                                });
                            }
                        } else if hit_test(toggle_rects[2], cx, cy) {
                            toggles.cycle_quality();
                            let (_, bitrate_cap_mbps) = QUALITY_PRESETS[toggles.quality_selected];
                            match negotiate_stream_url(
                                &emby,
                                &ids.user_id,
                                &ids.item_id,
                                bitrate_cap_mbps,
                                last_position_ticks,
                            )
                            .await
                            {
                                Ok((new_url, new_media_source_id)) => {
                                    stream_url = new_url;
                                    ids.media_source_id = new_media_source_id;
                                    let reapply_audio = toggles
                                        .audio_tracks
                                        .get(toggles.audio_selected)
                                        .map(|(id, _)| *id);
                                    if let Err(e) = restart_mpv_at(
                                        &mut child,
                                        &mut mpv,
                                        &config.socket_path,
                                        container.window,
                                        &config.frame_gen_settings,
                                        FRAME_GEN_BACKENDS[toggles.frame_gen_selected],
                                        &config.session_dir,
                                        &stream_url,
                                        !toggles.subtitle_options.is_empty(),
                                        last_position_ticks,
                                        reapply_audio,
                                    ) {
                                        tracing::warn!(error = %e, "failed to restart mpv for quality change");
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "failed to negotiate a new quality preset")
                                }
                            }
                        } else if hit_test(toggle_rects[3], cx, cy) {
                            toggles.cycle_frame_gen();
                            let new_backend = FRAME_GEN_BACKENDS[toggles.frame_gen_selected];
                            let reapply_audio = toggles
                                .audio_tracks
                                .get(toggles.audio_selected)
                                .map(|(id, _)| *id);
                            match restart_mpv_at(
                                &mut child,
                                &mut mpv,
                                &config.socket_path,
                                container.window,
                                &config.frame_gen_settings,
                                new_backend,
                                &config.session_dir,
                                &stream_url,
                                !toggles.subtitle_options.is_empty(),
                                last_position_ticks,
                                reapply_audio,
                            ) {
                                Ok(()) => {
                                    write_override(&ids.override_key, ids.item_type, |entry| {
                                        entry.frame_gen_backend = Some(new_backend);
                                    });
                                }
                                Err(e) => {
                                    tracing::warn!(error = %e, "failed to restart mpv for frame-gen change")
                                }
                            }
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "failed to poll overlay input events"),
            }
            if let Some(target) = switch_target {
                exit = SessionExit::SwitchItem(target);
                break 'ticks;
            }

            if let Ok(position_ticks) = mpv_ipc::current_position_ticks(&mpv) {
                last_position_ticks = position_ticks;
            }
            if let Ok(duration) = mpv_ipc::duration_seconds(&mpv) {
                last_duration_seconds = duration;
            }
            if let Ok(is_paused) = mpv_ipc::is_paused(&mpv) {
                last_is_paused = is_paused;
            }

            let mut frame = match &subtitle_renderer {
                Some(renderer) => renderer.render_rgba(last_position_ticks / 10_000),
                None => vec![0u8; (canvas_width * canvas_height * 4) as usize],
            };
            draw_back_button(&mut frame, canvas_width, canvas_height);
            if metadata.prev_item_id.is_some() {
                let (bx, by, bw, bh) = overlay_window::PREV_BUTTON;
                draw_button(
                    &mut frame,
                    canvas_width,
                    canvas_height,
                    (bx as i32, by as i32, bw as i32, bh as i32),
                    "PREV",
                );
            }
            if metadata.next_item_id.is_some() {
                let (bx, by, bw, bh) = overlay_window::NEXT_BUTTON;
                draw_button(
                    &mut frame,
                    canvas_width,
                    canvas_height,
                    (bx as i32, by as i32, bw as i32, bh as i32),
                    "NEXT",
                );
            }
            let toggle_rects = quick_toggle_rects(canvas_width, canvas_height);
            draw_button(
                &mut frame,
                canvas_width,
                canvas_height,
                toggle_rects[0],
                &toggles.subtitle_label(),
            );
            draw_button(
                &mut frame,
                canvas_width,
                canvas_height,
                toggle_rects[1],
                &toggles.audio_label(),
            );
            draw_button(
                &mut frame,
                canvas_width,
                canvas_height,
                toggle_rects[2],
                &toggles.quality_label(),
            );
            draw_button(
                &mut frame,
                canvas_width,
                canvas_height,
                toggle_rects[3],
                &toggles.frame_gen_label(),
            );
            if Instant::now() < osd_deadline {
                draw_osd(
                    &mut frame,
                    canvas_width,
                    canvas_height,
                    &metadata.title,
                    last_position_ticks as f64 / 10_000_000.0,
                    last_duration_seconds,
                    &metadata.chapters,
                );
            }
            if let Err(e) = overlay.paint(&frame) {
                tracing::warn!(error = %e, "failed to paint overlay");
            }

            if since_last_progress_report >= PROGRESS_REPORT_INTERVAL {
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
                since_last_progress_report = Duration::ZERO;
            }

            tokio::time::sleep(OVERLAY_TICK).await;
            since_last_progress_report += OVERLAY_TICK;
        }

        if let Err(e) = emby
            .report_stopped(&StoppedRequest {
                item_id: ids.item_id.clone(),
                media_source_id: ids.media_source_id.clone(),
                play_session_id: ids.play_session_id.clone(),
                position_ticks: last_position_ticks,
            })
            .await
        {
            tracing::warn!(error = %e, "failed to report playback stopped");
        }

        match exit {
            SessionExit::Stopped => break 'sessions,
            SessionExit::SwitchItem(new_item_id) => {
                let _ = child.kill();
                let _ = child.wait();
                let title_override = read_override(&ids.override_key);
                match load_item(
                    &emby,
                    &config,
                    title_override.as_ref(),
                    &ids.user_id,
                    &new_item_id,
                    ids.item_type,
                    &ids.override_key,
                    container.window,
                    last_canvas_size.0,
                    last_canvas_size.1,
                )
                .await
                {
                    Ok(loaded) => {
                        ids.item_id = new_item_id;
                        ids.media_source_id = loaded.media_source_id;
                        ids.play_session_id = loaded.play_session_id;
                        stream_url = loaded.stream_url;
                        subtitle_renderer = loaded.subtitle_renderer;
                        toggles = loaded.toggles;
                        metadata = loaded.metadata;
                        mpv = loaded.mpv;
                        child = loaded.child;
                        if let Err(e) = emby
                            .report_playing(&PlayingRequest {
                                item_id: ids.item_id.clone(),
                                media_source_id: ids.media_source_id.clone(),
                                play_session_id: ids.play_session_id.clone(),
                                position_ticks: 0,
                                is_paused: false,
                                can_seek: true,
                            })
                            .await
                        {
                            tracing::warn!(error = %e, "failed to report playback playing for the new episode");
                        }
                        continue 'sessions;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to load the next/previous episode, ending playback");
                        break 'sessions;
                    }
                }
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = overlay.destroy();
    let _ = container.destroy();
    let _ = std::fs::remove_file(&config.socket_path);

    // Best-effort — if there's no history to go back to (e.g. playback
    // was somehow triggered directly), fall back to the library home
    // page rather than leaving the webview stuck on the player route.
    let _ = main_window.eval(
        "if (window.history.length > 1) { window.history.back(); } else { location.hash = '#!/home.html'; }",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emby::models::ItemSummary;

    #[test]
    fn format_time_pads_minutes_and_seconds() {
        assert_eq!(format_time(65.4), "01:05");
        assert_eq!(format_time(0.0), "00:00");
        assert_eq!(format_time(3661.0), "61:01");
    }

    #[test]
    fn format_time_clamps_negative_to_zero() {
        assert_eq!(format_time(-5.0), "00:00");
    }

    fn item(
        name: &str,
        season: Option<i32>,
        episode: Option<i32>,
        series: Option<&str>,
    ) -> ItemSummary {
        ItemSummary {
            id: "id".to_string(),
            name: name.to_string(),
            index_number: episode,
            parent_index_number: season,
            series_name: series.map(str::to_string),
            chapters: Vec::new(),
        }
    }

    #[test]
    fn format_item_title_includes_series_and_episode_numbers() {
        let ep = item("Pilot", Some(1), Some(2), Some("Some Show"));
        assert_eq!(format_item_title(&ep), "Some Show S1E2 Pilot");
    }

    #[test]
    fn format_item_title_falls_back_to_just_the_name_for_movies() {
        let movie = item("A Great Movie", None, None, None);
        assert_eq!(format_item_title(&movie), "A Great Movie");
    }

    #[test]
    fn format_item_title_truncates_long_titles() {
        let long_name = "X".repeat(OSD_TITLE_MAX_CHARS + 20);
        let movie = item(&long_name, None, None, None);
        let title = format_item_title(&movie);
        assert!(title.ends_with("..."));
        assert_eq!(title.chars().count(), OSD_TITLE_MAX_CHARS + 3);
    }

    #[test]
    fn seek_bar_click_uses_raw_fraction_when_no_chapter_is_close() {
        let rect = (0, 0, 1000, 8);
        let seconds = seek_bar_click_seconds(rect, 100.0, &[], 500);
        assert_eq!(seconds, 50.0);
    }

    #[test]
    fn seek_bar_click_snaps_to_a_nearby_chapter() {
        let rect = (0, 0, 1000, 8);
        // Chapter at 50s sits at pixel 500; clicking at 505 should snap.
        let seconds = seek_bar_click_seconds(rect, 100.0, &[50.0], 505);
        assert_eq!(seconds, 50.0);
    }

    #[test]
    fn seek_bar_click_does_not_snap_when_far_from_any_chapter() {
        let rect = (0, 0, 1000, 8);
        let seconds = seek_bar_click_seconds(rect, 100.0, &[10.0], 500);
        assert_eq!(seconds, 50.0);
    }
}
