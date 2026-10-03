//! Handoff from the browse UI to the player: negotiates a stream with Emby,
//! loads it into mpv with the right tracks and SVP state, and keeps the
//! server's session/resume state current.

pub mod markers;
pub mod tracks;

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use gtk::glib;

use crate::config::{FrameGenBackend, Settings};
use crate::db::{Db, ItemType, TitleOverride};
use crate::emby::EmbyClient;
use crate::emby::models::{BaseItem, MediaSource, ProgressRequest, StoppedRequest};
use crate::emby::playback_info::StreamRequest;
use crate::frame_gen::resolve_backend;
use crate::player::{Player, Track, TrackKind};
use crate::runtime::{block_on_timeout, spawn_detached, spawn_tokio};
use crate::ui::Session;
use markers::{Markers, neighbours};
use tracks::{SUBTITLES_OFF, SubtitleStart};

/// Emby positions are in 100ns ticks.
pub const TICKS_PER_SECOND: i64 = 10_000_000;

const PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
/// How long closing the window may wait for the final Stopped report.
const STOP_REPORT_TIMEOUT: Duration = Duration::from_secs(2);

/// Stream quality: the original file, or a server transcode capped at a
/// bitrate (kbps). Picked per session, not remembered (it's about the
/// network); the configured default applies at the start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    #[default]
    Original,
    Kbps(u32),
}

/// The choices offered, best first (the same steps as Emby's web player).
pub const QUALITIES: [Quality; 13] = [
    Quality::Original,
    Quality::Kbps(40_000),
    Quality::Kbps(20_000),
    Quality::Kbps(10_000),
    Quality::Kbps(8_000),
    Quality::Kbps(6_000),
    Quality::Kbps(4_000),
    Quality::Kbps(3_000),
    Quality::Kbps(2_000),
    Quality::Kbps(1_500),
    Quality::Kbps(1_000),
    Quality::Kbps(720),
    Quality::Kbps(420),
];

impl Quality {
    /// "Original", "10 Mbps", "720p · 4 Mbps", "360p · 720 kbps".
    pub fn label(self) -> String {
        let Quality::Kbps(kbps) = self else {
            return "Original".to_string();
        };
        let rate = if kbps >= 1_000 {
            let mbps = f64::from(kbps) / 1_000.0;
            format!("{} Mbps", format!("{mbps:.1}").trim_end_matches(".0"))
        } else {
            format!("{kbps} kbps")
        };
        match self.max_height() {
            Some(height) => format!("{height}p · {rate}"),
            None => rate,
        }
    }

    /// From config's bitrate cap in kbps (0 = original).
    pub fn from_kbps(kbps: u32) -> Self {
        if kbps == 0 {
            Quality::Original
        } else {
            Quality::Kbps(kbps)
        }
    }

    pub fn kbps(self) -> u32 {
        match self {
            Quality::Original => 0,
            Quality::Kbps(kbps) => kbps,
        }
    }

    fn max_bitrate(self) -> Option<i64> {
        match self {
            Quality::Original => None,
            Quality::Kbps(kbps) => Some(i64::from(kbps) * 1_000),
        }
    }

    /// The tallest picture worth sending at this bitrate; above ~6 Mbps
    /// the source size is fine. Without a cap the server squeezes 1080p
    /// into the bitrate, which looks far worse than a smaller picture.
    pub fn max_height(self) -> Option<u32> {
        match self.kbps() {
            0 | 6_000.. => None,
            3_000..6_000 => Some(720),
            1_500..3_000 => Some(480),
            700..1_500 => Some(360),
            _ => Some(240),
        }
    }

    fn play_method(self) -> &'static str {
        match self {
            Quality::Original => "DirectStream",
            Quality::Kbps(_) => "Transcode",
        }
    }
}

/// A row in the audio/subtitle menus.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackEntry {
    pub id: i64,
    pub label: String,
    pub selected: bool,
}

/// One playing item, from load until it's stopped or replaced.
pub struct PlaybackSession {
    client: Arc<EmbyClient>,
    player: Player,
    /// The item with its chapters.
    pub item: BaseItem,
    pub markers: Markers,
    pub previous: Option<BaseItem>,
    pub next: Option<BaseItem>,
    pub quality: Quality,
    /// Episodes remember choices per series, everything else per item.
    override_key: String,
    override_type: ItemType,
    source: MediaSource,
    play_session_id: String,
    /// Emby subtitle streams loaded as separate files: (stream index, url).
    external_subtitles: Vec<(i32, String)>,
    last_ticks: Cell<i64>,
    timer: RefCell<Option<glib::SourceId>>,
    stopped: Cell<bool>,
    /// HDR source: SVP stays off unless the title explicitly enables it.
    pub hdr: bool,
    /// Shader groups found when playback started (none for music).
    shader_groups: Vec<crate::shaders::Group>,
}

impl PlaybackSession {
    pub async fn start(
        session: Session,
        player: Player,
        item: &BaseItem,
        start_ticks: i64,
        quality: Quality,
        audio_stream_index: Option<i32>,
    ) -> Result<Rc<Self>> {
        let client = session.client.clone();
        let user_id = session.user_id.clone();
        let item_id = item.id.clone();
        let series_id = item.series_id.clone();
        let request = StreamRequest {
            start_ticks,
            max_bitrate: quality.max_bitrate(),
            max_height: quality.max_height(),
            audio_stream_index,
        };
        let (item, episodes, info) = spawn_tokio({
            let client = client.clone();
            async move {
                let episodes = async {
                    match &series_id {
                        Some(series_id) => client.series_episodes(series_id, &user_id).await,
                        None => Ok(Vec::new()),
                    }
                };
                let (item, episodes, info) = tokio::join!(
                    client.item_with_chapters(&user_id, &item_id),
                    episodes,
                    client.get_playback_info(&user_id, &item_id, &request),
                );
                // Neighbours are a nicety; playback goes ahead without them.
                let episodes = episodes.unwrap_or_else(|e| {
                    tracing::warn!("episode list failed: {e:#}");
                    Vec::new()
                });
                Ok::<_, anyhow::Error>((item?, episodes, info?))
            }
        })
        .await?;

        let source = info
            .media_sources
            .into_iter()
            .next()
            .context("Emby returned no playable media source for this item")?;
        let mut external_audio = None;
        let mut web_captions = None;
        let url = match quality {
            // Trailers and other links elsewhere play from their own URL.
            _ if source.is_remote => {
                let path = source.path.clone().context("this item has no address")?;
                let options =
                    crate::remote::Options::from_settings(&Settings::load().unwrap_or_default());
                let stream = spawn_tokio(async move {
                    tokio::task::spawn_blocking(move || crate::remote::resolve(&path, &options))
                        .await?
                })
                .await?;
                external_audio = stream.audio;
                web_captions = stream.subtitle;
                stream.url
            }
            Quality::Original => client.direct_stream_url(&item.id, &source.id)?,
            Quality::Kbps(_) => client.resolve_transcoding_url(&without_burned_subtitles(
                source
                    .transcoding_url
                    .as_deref()
                    .context("Emby didn't offer a transcode for this item")?,
            )),
        };
        let external_subtitles = subtitle_files(&client, &item.id, &source, quality)?;
        let hdr = crate::emby::models::is_hdr(&source.media_streams);
        let (previous, next) = neighbours(&episodes, &item.id);
        let (override_key, override_type) = match &item.series_id {
            Some(series_id) => (series_id.clone(), ItemType::Series),
            None => (item.id.clone(), ItemType::Movie),
        };

        // Nothing to sharpen or upscale in music.
        let shader_groups = if item.is_audio() {
            Vec::new()
        } else {
            crate::shaders::groups()
        };
        let session = Rc::new(PlaybackSession {
            client,
            player,
            markers: Markers::from_chapters(&item.chapters),
            item,
            previous,
            next,
            quality,
            override_key,
            override_type,
            source,
            play_session_id: info.play_session_id,
            external_subtitles,
            last_ticks: Cell::new(start_ticks),
            timer: RefCell::new(None),
            stopped: Cell::new(false),
            hdr,
            shader_groups,
        });

        let settings = Settings::load().unwrap_or_default();
        // Nothing to interpolate in music.
        let svp = session.svp_enabled() && !session.item.is_audio();
        if svp && settings.frame_gen.auto_start_svp() {
            crate::svp::ensure_running();
        }
        player.set_svp(settings.frame_gen.socket(), svp)?;
        apply_smoothing(player, svp);
        if let Err(e) = player.apply_video(&settings.video) {
            tracing::warn!("{e:#}");
        }
        session.apply_shaders();
        let subtitle_urls: Vec<&str> = session
            .external_subtitles
            .iter()
            .map(|(_, url)| url.as_str())
            .chain(web_captions.as_deref())
            .collect();
        player.load_at(
            &url,
            start_ticks as f64 / TICKS_PER_SECOND as f64,
            &subtitle_urls,
            external_audio.as_deref(),
        )?;
        session.report_playing();

        let weak = Rc::downgrade(&session);
        let timer = glib::timeout_add_local(PROGRESS_INTERVAL, move || match weak.upgrade() {
            Some(session) => {
                session.report_progress(Some("TimeUpdate"));
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        session.timer.replace(Some(timer));
        Ok(session)
    }

    /// Picks the starting audio/subtitle tracks once mpv has loaded the
    /// file: the title's remembered choice, else the configured languages.
    /// The Emby media source, for its trickplay thumbnails; `None` for web
    /// links, which have none.
    pub fn trickplay_source(&self) -> Option<&str> {
        (!self.source.is_remote).then_some(self.source.id.as_str())
    }

    pub fn apply_initial_tracks(&self) {
        // Web trailers come with their tracks chosen (captions as set in
        // Preferences); Emby knows nothing about them.
        if self.source.is_remote {
            return;
        }
        let tracks = self.player.tracks();
        let remembered = self.remembered();
        let settings = Settings::load().unwrap_or_default();
        let audio = tracks::initial_audio(
            &tracks,
            remembered
                .as_ref()
                .and_then(|o| o.audio_language.as_deref()),
            &settings.audio.preferred_language,
        );
        if let Some(id) = audio
            && let Err(e) = self.player.set_audio(id)
        {
            tracing::warn!("{e:#}");
        }
        let subtitle = tracks::initial_subtitle(
            &tracks,
            remembered
                .as_ref()
                .and_then(|o| o.subtitle_language.as_deref()),
            remembered
                .as_ref()
                .and_then(|o| o.subtitle_forced_only)
                .unwrap_or(false),
            &settings.subtitles.preferred_language,
        );
        let result = match subtitle {
            SubtitleStart::Keep => Ok(()),
            SubtitleStart::Off => self.player.set_subtitle(None),
            SubtitleStart::Select(id) => self.player.set_subtitle(Some(id)),
        };
        if let Err(e) = result {
            tracing::warn!("{e:#}");
        }
    }

    /// Menu entries for one track kind, labelled from Emby when possible.
    pub fn track_entries(&self, kind: TrackKind) -> Vec<TrackEntry> {
        self.player
            .tracks()
            .into_iter()
            .filter(|t| t.kind == Some(kind))
            .map(|track| TrackEntry {
                id: track.id,
                label: tracks::track_label(&track, self.emby_stream(&track)),
                selected: track.selected,
            })
            .collect()
    }

    fn emby_stream(&self, track: &Track) -> Option<&crate::emby::models::MediaStream> {
        // A transcode's HLS stream indexes don't match the source file's.
        if self.quality != Quality::Original && !track.external {
            return None;
        }
        tracks::emby_stream(track, &self.source.media_streams, &self.external_subtitles)
    }

    /// The user picked an audio track: switch, remember its language for
    /// this title, tell Emby.
    pub fn select_audio(&self, id: i64) {
        if let Err(e) = self.player.set_audio(id) {
            tracing::warn!("{e:#}");
            return;
        }
        let lang = self.track(TrackKind::Audio, id).and_then(|t| t.lang);
        if lang.is_some() {
            self.remember(|o| o.audio_language = lang);
        }
        self.report_progress(Some("AudioTrackChange"));
    }

    /// `None` turns subtitles off (remembered as such).
    pub fn select_subtitle(&self, id: Option<i64>) {
        if let Err(e) = self.player.set_subtitle(id) {
            tracing::warn!("{e:#}");
            return;
        }
        let track = id.and_then(|id| self.track(TrackKind::Subtitle, id));
        let language = match &track {
            Some(track) => track.lang.clone(),
            None => Some(SUBTITLES_OFF.to_string()),
        };
        if language.is_some() {
            let forced = track.as_ref().is_some_and(|t| t.forced);
            self.remember(|o| {
                o.subtitle_language = language;
                o.subtitle_forced_only = Some(forced);
            });
        }
        self.report_progress(Some("SubtitleTrackChange"));
    }

    fn track(&self, kind: TrackKind, id: i64) -> Option<Track> {
        self.player
            .tracks()
            .into_iter()
            .find(|t| t.kind == Some(kind) && t.id == id)
    }

    /// Emby `MediaStream.Index` of the playing audio, to keep it across a
    /// quality change.
    pub fn audio_stream_index(&self) -> Option<i32> {
        let track = self
            .player
            .tracks()
            .into_iter()
            .find(|t| t.kind == Some(TrackKind::Audio) && t.selected)?;
        self.emby_stream(&track).map(|s| s.index)
    }

    /// The title's remembered picture aspect and extra zoom.
    pub fn picture(&self) -> (crate::player::Aspect, f64) {
        let remembered = self.remembered();
        let aspect = remembered
            .as_ref()
            .and_then(|o| o.aspect_mode.as_deref())
            .and_then(crate::player::Aspect::from_name)
            .unwrap_or_default();
        (aspect, remembered.and_then(|o| o.zoom).unwrap_or(0.0))
    }

    pub fn remember_picture(&self, aspect: crate::player::Aspect, zoom: f64) {
        self.remember(|o| {
            o.aspect_mode = Some(aspect.name().to_string());
            o.zoom = Some(zoom);
        });
    }

    pub fn svp_enabled(&self) -> bool {
        let default = Settings::load()
            .unwrap_or_default()
            .frame_gen
            .default_backend;
        let remembered = self.remembered().and_then(|o| o.frame_gen_backend);
        if self.hdr {
            // SVP's 8-bit path bands HDR and costs a lot at 4K, so HDR needs
            // an explicit opt-in per title.
            return remembered == Some(FrameGenBackend::Svp);
        }
        resolve_backend(default, remembered) == FrameGenBackend::Svp
    }

    pub fn set_svp(&self, enabled: bool) {
        if let Err(e) = self.player.set_svp(&svp_socket(), enabled) {
            tracing::warn!("{e:#}");
            return;
        }
        apply_smoothing(self.player, enabled);
        let backend = if enabled {
            FrameGenBackend::Svp
        } else {
            FrameGenBackend::Off
        };
        self.remember(|o| o.frame_gen_backend = Some(backend));
    }

    /// The shader groups shown in the player, each with its preset in
    /// effect for this title (`None`: off).
    pub fn shaders(&self) -> Vec<(crate::shaders::Group, Option<String>)> {
        let settings = Settings::load().unwrap_or_default().shaders;
        let title = self.shader_choices();
        crate::shaders::active(&self.shader_groups, &settings, &title)
            .into_iter()
            .map(|(group, preset)| (group.clone(), preset.map(|p| p.id.clone())))
            .collect()
    }

    /// Picks `preset` ("" for off) for `group`, for this title from now on.
    pub fn set_shader(&self, group: &str, preset: &str) {
        let mut choices = self.shader_choices();
        choices.insert(group.to_string(), preset.to_string());
        self.remember(|o| o.shaders = Some(crate::shaders::encode_choices(&choices)));
        self.apply_shaders();
    }

    fn shader_choices(&self) -> crate::shaders::Choices {
        self.remembered()
            .and_then(|o| o.shaders)
            .map(|text| crate::shaders::decode_choices(&text))
            .unwrap_or_default()
    }

    fn apply_shaders(&self) {
        let settings = Settings::load().unwrap_or_default().shaders;
        let title = self.shader_choices();
        let active = crate::shaders::active(&self.shader_groups, &settings, &title);
        if let Err(e) = self.player.set_shaders(&crate::shaders::chain(&active)) {
            tracing::warn!("{e:#}");
        }
    }

    fn remembered(&self) -> Option<TitleOverride> {
        Db::open_default()
            .and_then(|db| db.get_override(&self.override_key))
            .unwrap_or_else(|e| {
                tracing::warn!("overrides unreadable: {e:#}");
                None
            })
    }

    fn remember(&self, change: impl FnOnce(&mut TitleOverride)) {
        let mut entry = self.remembered().unwrap_or_else(|| TitleOverride {
            emby_item_id: self.override_key.clone(),
            item_type: self.override_type,
            audio_language: None,
            subtitle_language: None,
            subtitle_forced_only: None,
            frame_gen_backend: None,
            frame_gen_multiplier: None,
            aspect_mode: None,
            zoom: None,
            shaders: None,
        });
        change(&mut entry);
        if let Err(e) = Db::open_default().and_then(|db| db.upsert_override(&entry)) {
            tracing::warn!("could not save the title override: {e:#}");
        }
    }

    pub fn position_ticks(&self) -> i64 {
        if let Some(seconds) = self.player.position() {
            self.last_ticks.set(seconds_to_ticks(seconds));
        }
        self.last_ticks.get()
    }

    /// Reports the current state; `event` is Emby's `EventName`
    /// (`TimeUpdate`, `Pause`, `Unpause`, `VolumeChange`, ...).
    pub fn report_progress(&self, event: Option<&'static str>) {
        if self.stopped.get() {
            return;
        }
        let request = self.progress_request(event);
        let client = self.client.clone();
        spawn_detached(async move {
            if let Err(e) = client.report_progress(&request).await {
                tracing::warn!("progress report failed: {e:#}");
            }
        });
    }

    fn report_playing(&self) {
        let request = self.progress_request(None);
        let client = self.client.clone();
        spawn_detached(async move {
            if let Err(e) = client.report_playing(&request).await {
                tracing::warn!("playing report failed: {e:#}");
            }
        });
    }

    fn progress_request(&self, event: Option<&'static str>) -> ProgressRequest {
        let selected = |kind| {
            let track = self
                .player
                .tracks()
                .into_iter()
                .find(|t| t.kind == Some(kind) && t.selected)?;
            self.emby_stream(&track).map(|s| s.index)
        };
        ProgressRequest {
            item_id: self.item.id.clone(),
            media_source_id: self.source.id.clone(),
            play_session_id: self.play_session_id.clone(),
            position_ticks: self.position_ticks(),
            is_paused: self.player.is_paused(),
            is_muted: self.player.is_muted(),
            volume_level: self.player.volume().round() as i32,
            can_seek: true,
            play_method: self.quality.play_method(),
            audio_stream_index: selected(TrackKind::Audio),
            // Emby uses -1 for "subtitles off".
            subtitle_stream_index: selected(TrackKind::Subtitle).or(Some(-1)),
            event_name: event,
        }
    }

    /// Stops mpv and tells Emby where playback ended, which becomes the
    /// item's resume point (or marks it watched near the end). Safe to
    /// call more than once.
    pub fn stop(&self) {
        if let Some(report) = self.stop_without_reporting() {
            spawn_detached(report);
        }
    }

    /// [`stop`](Self::stop), but waits (briefly) for the report to reach
    /// Emby, for when the app is about to exit.
    pub fn stop_and_wait(&self) {
        if let Some(report) = self.stop_without_reporting()
            && block_on_timeout(report, STOP_REPORT_TIMEOUT).is_none()
        {
            tracing::warn!("stopped report timed out on exit");
        }
    }

    /// Stops mpv; returns the pending Stopped report the first time.
    fn stop_without_reporting(&self) -> Option<impl Future<Output = ()> + Send + 'static> {
        if self.stopped.replace(true) {
            return None;
        }
        if let Some(timer) = self.timer.take() {
            timer.remove();
        }
        let position_ticks = self.position_ticks();
        if let Err(e) = self.player.stop() {
            tracing::warn!("{e:#}");
        }
        let request = StoppedRequest {
            item_id: self.item.id.clone(),
            media_source_id: self.source.id.clone(),
            play_session_id: self.play_session_id.clone(),
            position_ticks,
        };
        let client = self.client.clone();
        Some(async move {
            if let Err(e) = client.report_stopped(&request).await {
                tracing::warn!("stopped report failed: {e:#}");
            }
        })
    }
}

/// Subtitles mpv should load as separate files. A direct stream already
/// carries embedded tracks, so only Emby's external files are added; a
/// transcode carries none, so every text track is fetched as a file.
fn subtitle_files(
    client: &EmbyClient,
    item_id: &str,
    source: &MediaSource,
    quality: Quality,
) -> Result<Vec<(i32, String)>> {
    let mut files = Vec::new();
    for stream in &source.media_streams {
        if stream.stream_type != "Subtitle" {
            continue;
        }
        let wanted = match quality {
            Quality::Original => stream.is_external,
            Quality::Kbps(_) => stream.is_external || stream.is_text_subtitle_stream,
        };
        if !wanted {
            continue;
        }
        let url = match (&stream.delivery_url, stream.is_external) {
            (Some(delivery_url), true) => client.resolve_transcoding_url(delivery_url),
            _ => {
                let format = subtitle_format(stream.codec.as_deref());
                client.subtitle_url(item_id, &source.id, stream.index, format)?
            }
        };
        files.push((stream.index, url));
    }
    Ok(files)
}

/// Emby ignores `SubtitleStreamIndex: -1` and still burns the default
/// subtitle into the transcode (`SubtitleStreamIndex=N&SubtitleMethod=Encode`
/// in the URL), which doubles up with the track mpv renders. Dropping the
/// two parameters gets a clean picture.
fn without_burned_subtitles(url: &str) -> String {
    let Some((path, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|param| {
            let key = param.split('=').next().unwrap_or_default();
            !key.eq_ignore_ascii_case("SubtitleStreamIndex")
                && !key.eq_ignore_ascii_case("SubtitleMethod")
        })
        .collect();
    format!("{path}?{}", kept.join("&"))
}

/// File extension Emby serves a subtitle codec as.
fn subtitle_format(codec: Option<&str>) -> &'static str {
    match codec.map(str::to_ascii_lowercase).as_deref() {
        Some("ass") => "ass",
        Some("ssa") => "ssa",
        Some("webvtt" | "vtt") => "vtt",
        _ => "srt",
    }
}

/// "Smooth motion without SVP": mpv's frame blending, only for titles with
/// SVP off. With the setting off it's never touched (only switched back off
/// if an earlier playback turned it on).
fn apply_smoothing(player: Player, svp: bool) {
    let wanted = Settings::load()
        .unwrap_or_default()
        .frame_gen
        .smooth_without_svp
        && !svp;
    if (wanted || player.smooth_motion())
        && let Err(e) = player.set_smooth_motion(wanted)
    {
        tracing::warn!("{e:#}");
    }
}

pub(crate) fn svp_socket() -> String {
    Settings::load()
        .unwrap_or_default()
        .frame_gen
        .socket()
        .to_string()
}

fn seconds_to_ticks(seconds: f64) -> i64 {
    (seconds * TICKS_PER_SECOND as f64) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> MediaSource {
        serde_json::from_value(serde_json::json!({
            "Id": "ms1",
            "SupportsDirectPlay": true,
            "SupportsDirectStream": true,
            "SupportsTranscoding": true,
            "MediaStreams": [
                {"Index": 0, "Type": "Video", "Codec": "hevc"},
                {"Index": 2, "Type": "Subtitle", "Codec": "ass", "IsTextSubtitleStream": true},
                {"Index": 3, "Type": "Subtitle", "Codec": "PGSSUB"},
                {"Index": 4, "Type": "Subtitle", "Codec": "subrip", "IsExternal": true,
                 "IsTextSubtitleStream": true, "DeliveryUrl": "/Videos/i/ms1/Subtitles/4/Stream.srt"}
            ]
        }))
        .unwrap()
    }

    fn client() -> EmbyClient {
        EmbyClient::new("http://server:8096", "device").with_token("tok")
    }

    #[test]
    fn direct_stream_only_adds_external_subtitles() {
        let files = subtitle_files(&client(), "i", &source(), Quality::Original).unwrap();
        assert_eq!(
            files,
            [(
                4,
                "http://server:8096/Videos/i/ms1/Subtitles/4/Stream.srt".to_string()
            )]
        );
    }

    #[test]
    fn transcode_fetches_every_text_subtitle() {
        let files = subtitle_files(&client(), "i", &source(), Quality::Kbps(6_000)).unwrap();
        let indexes: Vec<i32> = files.iter().map(|(i, _)| *i).collect();
        assert_eq!(indexes, [2, 4]);
        assert_eq!(
            files[0].1,
            "http://server:8096/emby/Videos/i/ms1/Subtitles/2/Stream.ass?api_key=tok"
        );
    }

    #[test]
    fn burned_in_subtitle_params_are_dropped() {
        assert_eq!(
            without_burned_subtitles(
                "/videos/1/master.m3u8?DeviceId=d&SubtitleStreamIndex=2&SubtitleMethod=Encode&api_key=k"
            ),
            "/videos/1/master.m3u8?DeviceId=d&api_key=k"
        );
        assert_eq!(
            without_burned_subtitles("/videos/1/master.m3u8"),
            "/videos/1/master.m3u8"
        );
    }

    #[test]
    fn quality_labels_and_bitrates() {
        assert_eq!(Quality::Original.label(), "Original");
        assert_eq!(Quality::from_kbps(0), Quality::Original);
        assert_eq!(Quality::from_kbps(6_000).kbps(), 6_000);
        assert_eq!(Quality::Kbps(6_000).max_bitrate(), Some(6_000_000));
        assert_eq!(Quality::Kbps(420).max_bitrate(), Some(420_000));
        assert_eq!(Quality::Original.play_method(), "DirectStream");
        assert_eq!(Quality::Kbps(3_000).play_method(), "Transcode");
        assert_eq!(Quality::Kbps(10_000).label(), "10 Mbps");
        assert_eq!(Quality::Kbps(4_000).label(), "720p · 4 Mbps");
        assert_eq!(Quality::Kbps(1_500).label(), "480p · 1.5 Mbps");
        assert_eq!(Quality::Kbps(720).label(), "360p · 720 kbps");
        assert_eq!(Quality::Kbps(420).label(), "240p · 420 kbps");
        assert_eq!(Quality::Kbps(6_000).max_height(), None);
        assert_eq!(Quality::Original.max_height(), None);
    }

    #[test]
    fn seconds_convert_to_ticks() {
        assert_eq!(seconds_to_ticks(1.5), 15_000_000);
    }
}
