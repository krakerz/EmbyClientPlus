# Changelog

## [Unreleased]

### Added
- Project scaffolding: Tauri app shell, build tooling, CI skeleton
- Global settings (TOML), keyring-backed auth token storage with plaintext fallback, and a local SQLite store for per-series/movie playback overrides
- Emby API client: authentication fallback, PlaybackInfo negotiation with a permissive device profile, session progress reporting, and series-id resolution for episodes
- Embedded Emby web client in the main window (first-run "connect to server" form, then loads the configured server directly) with a route-interception script for handing playback off to the native player
- Playback orchestration: intercepted playback now negotiates PlaybackInfo, resolves per-series/movie overrides, and spawns mpv embedded in its own window with real Sessions/Playing progress reporting
