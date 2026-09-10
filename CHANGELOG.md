# Changelog

## [Unreleased]

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
