# Changelog

## [Unreleased]

### Added
- Log files: each launch writes `~/.config/embyclientplus/logs/embyclientplus-YYYYMMDD-HHMMSS.log` (UTC); the newest 10 are kept. mpv's own messages and panics are included.
- Preferences (Home menu): SVP by default, default quality, preferred audio and subtitle languages, forget all per-title choices, open the logs folder.
- The SVP button shows what SVP is doing: green while interpolating, amber while waiting for SVP Manager, red (plus a one-time notice) when SVP Manager isn't running; disabled when SVP isn't installed.
- Posters are cached on disk (`~/.cache/embyclientplus/images`, trimmed to 512 MB), so they show instantly across launches.

### Changed
- Search results keep the server's relevance order instead of A–Z.
- The chosen quality carries over to the next episode.

### Fixed
- Doubled, overlapping subtitles on transcoded quality presets: Emby burned the subtitle into the video even when asked not to; the burn-in parameters are now stripped from the transcode URL.
- Crash (abort) when changing quality or jumping to the previous/next episode from the player.
- Closing the window during playback now waits briefly so Emby gets the final position.
- Volume changes no longer send a report per step.

## [0.4.0] — 2026-10-02

### Added
- Native player controls: auto-hiding OSD (hides after 3 s without mouse movement while playing; cursor hides too; stays while paused or a menu is open).
- OSD top bar: back button and title (series name for episodes / "S1:E4 · Episode" for movies).
- OSD bottom bar: seek bar with chapter marks and hover time, elapsed/remaining time, previous/play-pause/next episode buttons, volume popover (0–130% + mute), Audio menu, Subtitles menu (with Off), Quality menu, SVP toggle button, fullscreen button.
- Audio and subtitle choices are remembered per series/movie and restored on the next episode; otherwise config `audio.preferred_language` / `subtitles.preferred_language` apply.
- External Emby subtitle files loaded automatically via `sub-add`.
- Quality presets: Original (direct stream, default), 20, 10, 6, 3 Mbps; capped presets transcode and resume at the same position with audio track and subtitle selections preserved.
- Skip Intro and Skip Credits buttons (from Emby's IntroStart/IntroEnd/CreditsStart markers).
- Up Next card with 10 s countdown near episode end (from credits marker or last 20 s); autoplays next episode unless cancelled.
- SVP toggle per title with global default in config.
- Keyboard controls: Space/K play-pause, ←/→ seek 10 s, ↑/↓ volume, M mute, N/P next/previous episode, Page Down/Up next/previous chapter, F/F11 fullscreen, Esc leave fullscreen then back, double-click fullscreen.
- Playback reports: play method, audio/subtitle stream index, volume, mute status, and events for Pause/Unpause/track change/volume change/quality change; progress every 5 s.

### Changed
- SVP is now on by default (`frame_gen.default_backend = "svp"`).
- Seeking with ←/→ moves 10 s (was 5 s).
- Home rows can be dragged sideways with the mouse (touch-style scrolling).
- Row headings link to more: "Continue Watching >" and "Next Up >" open full grids, "Latest in X >" opens that library.
- Movie and TV libraries open as tabbed pages: Movies/Shows (sort by name, date added, release date, rating, runtime, random; ascending/descending; filter all/unplayed/played/in progress/favorites; item count), Suggestions (continue watching, next up, latest, "Because you watched…" rows), Collections (movies), Genres, Tags, Favorites, Folders.

### Fixed
- Playback failed with "Error when loading first segment": Emby fell back to an HLS transcode because the device profile only declared external subtitles. The profile now declares embedded subtitles for all formats, and playback always uses the direct stream.

## [0.3.0] — 2026-10-02

### Added
- Emby login screen with server address, username, and password
- Token storage in system keyring with plaintext fallback
- Auto-login on next launch
- Home page with Libraries, Continue Watching, Next Up, and Latest items per library
- Library poster grid with lazy-loading pagination
- Search page for movies, series, and episodes
- Series page with season dropdown and episode list (watched marks and progress bars)
- Item details page with Resume position and Play buttons
- Playback inside the app window with direct stream preferred and server transcode fallback
- Player controls: F11/f or double-click for fullscreen, Esc to exit fullscreen and go back, space to pause, arrow keys to seek by 5 seconds
- Progress reporting to Emby every 10 seconds and on playback stop
- Log Out option in the Home menu

## [0.2.0] — 2026-10-02

### Changed
- Rebuilt as a native GTK4/libadwaita app with the video player embedded in the window
- Bundles its own libmpv build with VapourSynth support for SVP

### Removed
- Embedded Emby web interface (native browsing comes in a later release)
- lsfg-vk frame generation backend
- Custom subtitle overlay (mpv renders subtitles directly)

## [0.1.0] — 2026-09-10

### Added
- Project scaffolding: Tauri app shell, build tooling, CI skeleton
- Global settings (TOML), keyring-backed auth token storage with plaintext fallback, and a local SQLite store for per-series/movie playback overrides
- Emby API client: authentication fallback, PlaybackInfo negotiation with a permissive device profile, session progress reporting, and series-id resolution for episodes
- Embedded Emby web client in the main window (first-run "connect to server" form, then loads the configured server directly) with a route-interception script for handing playback off to the native player
- Playback orchestration: intercepted playback now negotiates PlaybackInfo, resolves per-series/movie overrides, and spawns mpv embedded in its own window with real Sessions/Playing progress reporting
- Subtitle rendering: text-based subtitle tracks (SRT/ASS/SSA/VTT) are fetched and rendered independently of mpv via libass, composited onto a transparent overlay window above the video
- Native player overlay now follows the main window's position and size live, and includes a real back-to-library button that stops playback and returns to the previous page
- Quick-toggle buttons for subtitle, audio, quality, and frame-generation backend, right in the player overlay — audio/subtitle switch instantly, quality/frame-gen restart playback in place at the same position; per-title choices are remembered for next time
- Click-to-seek progress bar with chapter markers, an on-screen title/time display that fades out after a few seconds of inactivity, and previous/next-episode buttons for series playback

### Fixed
- lsfg-vk frame generation now actually activates — previously it silently never engaged (the process-identifier used to scope it to this app's own playback was too long and got truncated by the OS, so it never matched)
