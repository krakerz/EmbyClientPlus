# Changelog

## [Unreleased]

## [1.1.0] — 2026-10-03

### Added
- Update downloads show their progress (percentage and size) in the update notice and in Preferences → Updates.
- Command line: `-h`/`--help` and `-v`/`--version`.
- Long titles in the mini player, music panel and queue scroll (marquee) instead of being cut off; queue rows only while they have the cursor or pointer.

### Changed
- The mini player and music panel keep a fixed width, whatever the titles.

### Fixed
- Controller: the cursor jumped elsewhere after actions that rebuild a list or page (next/previous track, marking watched, page reloads); it now stays on the same spot.
- Controller: switching tabs (library tabs, Preferences) remembers each tab's own cursor position.

## [1.0.0] — 2026-10-03

First stable release.

### Added
- Reorder the music queue by dragging rows with the mouse.

### Changed
- Queue items can move past the playing track (buttons, controller X-move and drag); the playing track keeps playing, and it can be moved too.
- The music bar is a compact floating mini player at the bottom right again; the music panel opens upward from it at the same width, with Now Playing above the queue.
- The controller legend stays at the bottom left; when the mini player leaves too little room, its hints scroll slowly on one line instead of being cut off.

## [0.8.0] — 2026-10-03

### Added
- Music panel for controllers: LT/RT switch between the library and the music panel; in the panel Y plays/pauses, LB/RB change track, and X picks up a queue item to move with ↑/↓ (A/B/X to drop). Remappable under Preferences → Controller → In the Music Player.
- Quit button in the Preferences header; LB/RB switch Preferences tabs.

### Changed
- The music panel is full width, with Now Playing on the left and the queue on the right.
- The controller legend only shows after a controller press; mouse or touch input hides it. It has a solid background and sits at the bottom of the window (just above the music bar while music plays).
- LB/RB between episodes keep the cursor on the same spot (Play, or the previous/next episode link).

### Fixed
- Controller: Up from the first episode in a list (and similar list edges) did nothing.
- Controller: series pages now reliably start the cursor on the first (or last opened) episode.
- Controller: the cursor could land on nothing while a page was still loading.
- Controller: the cursor could get lost or stuck in the music panel; closing the panel returns it to where it was.
- Focused cards in library grids showed a double outline.
- SVP Manager is now stopped cleanly when the app is closed by Steam ("Exit game") or a termination signal, not only from the window.

## [0.7.0] — 2026-10-03

### Added
- More quality presets: 40, 8, 4, 2, 1.5 and 1 Mbps, 720 and 420 kbps; low ones also cap the resolution (720p down to 240p).
- Controller legend at the bottom left showing what the buttons do, following your mapping (hidden in the player).
- Episode pages: LB/RB open the previous or next episode.

### Changed
- SVP Manager starts with the app when auto-start is on; in Steam Game Mode the app opens once SVP Manager is up, so SVP no longer takes focus.
- The highlighted choice in player and card menus stands out more.

### Fixed
- Stutter during playback in Steam Game Mode, caused by running SVP Manager inside a hidden gamescope.
- Controller: Down did nothing right after launch, and the cursor went missing after a page reloaded.
- Controller: changing quality while moving through the player's buttons (Start) dropped back to seeking.

## [0.6.0] — 2026-10-02

### Added
- Dedicated music player (never uses SVP): mini player bar under every page, Now Playing sheet (cover, artist/album links, seek, shuffle, repeat off/all/one, volume), queue with play-now, move up/down, remove, clear upcoming; gapless playback.
- Album Play/Shuffle; track and card menus: Play, Shuffle, Instant Mix, Play Next, Add to Queue, Add to Playlist…
- Emby playlists: Playlists tab in music libraries, playlist page (play, shuffle, reorder, remove), Add-to-playlist dialog with "New playlist".
- MPRIS: desktop media controls and media keys.
- Black-bar fill in the player: Off / Blurred picture / Edge glow, for side and top/bottom bars (e.g. 1920×1200 handhelds); subtitles stay inside the picture.
- Picture menu: aspect (Fit, Fill, Stretch, 16:9, 4:3, 21:9) and zoom, remembered per title.
- Volume pop-up when the volume changes (keyboard or controller).
- TV Suggestions built from your library: "Because you watched …" and "More {genre}".
- Placeholder icons per item type for items without artwork.
- Theme setting: Follow system / Light / Dark (default Dark).
- App version shown in the Preferences header.
- Search starts in Home's header; Enter or the search button opens the results, with the cursor on the first result.
- Controller: Left/Right change settings in Preferences (options, numbers, switches, sliders); A steps through a setting's options.
- Seek-bar preview: the frame at that point (Emby's trickplay thumbnails, else chapter images), chapter name and time, on hover, drag and controller/keyboard seeks.
- Controller: Start in the player moves through the OSD buttons (D-pad/stick to move, A to press or open a menu, B or Start to leave); the OSD stays up meanwhile.
- Preferences: trailer quality (best available by default, or capped at 4K/1440p/1080p/720p) and trailer captions in your subtitle language (on by default).
- Controller: Select in the player skips the intro or credits when the Skip button shows.
- Search results in tabs: Top Results, Movies, Shows, Episodes, People, Collections, Albums, Songs and Artists (tabs without matches stay hidden; LB/RB switch tabs).
- Library grids remember their sort, order and filter.
- Sign-in page shows the app icon; the server address field hints "https://your-server-url:8920".

### Changed
- Holding Left/Right in the player for 1 s scrubs along the seek bar with the preview; the seek lands 2 s after the last move, instead of seeking on every repeat. A tap still seeks right away.
- HDR titles start with SVP off unless turned on for that title.
- In Steam Game Mode, SVP Manager runs inside a headless gamescope so it no longer steals focus.
- Controller focus ring is thicker and in the accent colour; focused cards also highlight their title.
- Series pages put the cursor on episode 1 (or the episode you last opened) so the controller starts there.
- The app is now called "Emby Client+" (window title, launcher, Emby's device list, media controls).

### Fixed
- The player's volume control showed 0% until the volume changed.
- Watched items you'd partly rewatched restarted from the beginning instead of resuming.
- The AppImage showed a light theme on Steam Deck.
- Controller: a second press of a player button (e.g. Y for subtitles) closed the menu by going Home; player menus now keep the controller inside the player.
- YouTube trailers failed with "Requested format is not available"; they now play as separate video and audio streams.
- Crash when starting some 10-bit videos on AMD RDNA4 GPUs: hardware decoding now prefers VA-API over Vulkan video.
- The window's maximize button icon was off-centre.

## [0.5.0] — 2026-10-02

### Added
- Music: music libraries open with Albums, Artists, Songs, Latest, Favorites and Folders tabs; album pages list tracks (disc/track numbers, durations) with Play; artist pages show their albums; tracks play in album order and the player shows the album cover. SVP stays off for music.
- Favorites split by kind: a Favorites page from Home (Series, Movies, Episodes, Albums, Songs, Artists) and per-library Favorites tabs with the kinds that library holds; each row opens the full sortable grid.
- Trailers: trailer items and a Trailer button on movie pages. YouTube trailers play in the app when yt-dlp is installed, otherwise they open in the browser.
- Lucide icon set everywhere, including GTK's and libadwaita's own icons (back arrow, window buttons, dropdowns); fixes the missing Controller tab icon.
- Genre and tag tabs show wide tiles with the name over a cover; the cover cycles through random titles from the category while hovered or focused.
- Folder cards without their own image show their first title's cover.
- Preferences > Home: Continue Watching / Next Up artwork can use series art instead of episode stills (no spoilers).
- Preferences > Display: window width/height and start fullscreen (Auto = fullscreen in Steam Game Mode only).
- Preferences > Playback: Use my mpv.conf — loads ~/.config/mpv/mpv.conf (shaders, scalers, subtitle styling); options the app manages (socket, video output, window, hwdec while SVP is on) are ignored and logged.
- Self-update: Preferences > Updates (and a notice at start-up) download the latest published release and replace the installed copy or AppImage, then offer a restart.
- Release archive with install.sh / uninstall.sh (installs to ~/.local/share/embyclientplus, adds a menu entry and ~/.local/bin/embyclientplus; uninstall keeps settings).
- Home button next to Back on every page; Home always refreshes when you return to it.
- Mark watched/unwatched and favourite from details and series pages, and from a right-click / long-press menu on any card (Play, Open, watched, favourite, Go to series).
- Cast & Crew row (opens everything a person appears in) and a More Like This row on details and series pages.
- Season chips replace the season dropdown.
- SVP socket path setting (default `/tmp/mpvsocket`).
- Gamescope / Steam Game Mode support: the app detects gamescope, runs fullscreen, and works around gamescope's Vulkan layer (GL renderer on Xwayland).
- SVP in Game Mode: the app starts SVP Manager itself when SVP is wanted and it isn't running (Preferences > Start SVP Manager automatically; on by default in gamescope) and stops it on exit.
- Smooth motion without SVP: optional mpv frame blending when SVP isn't interpolating.
- Controller support (gilrs): navigate everything with the D-pad/left stick, A select, B back, Y home, X item menu, LB/RB tabs or seasons, Select search, Start preferences; in the player A play/pause, D-pad seek/volume, LB/RB episodes, LT/RT chapters, X/Y audio/subtitle menus. Every action is remappable in Preferences > Controller.
- Release builds: GitHub Actions builds a draft release with a .tar.gz and an AppImage (bundles GTK4, libadwaita, FFmpeg and our libmpv; uses SVP's VapourSynth from the SVP folder when present).
- Log files: each launch writes `~/.config/embyclientplus/logs/embyclientplus-YYYYMMDD-HHMMSS.log` (UTC); the newest 10 are kept. mpv's own messages and panics are included.
- Preferences (Home menu): SVP by default, default quality, preferred audio and subtitle languages, forget all per-title choices, open the logs folder.
- Preferences: choose the SVP folder (default `~/SVP4`).
- The SVP button shows what SVP is doing: green while interpolating, amber while waiting for SVP Manager, red (plus a one-time notice) when SVP Manager isn't running; disabled when SVP isn't installed.
- Posters are cached on disk (`~/.cache/embyclientplus/images`, trimmed to 512 MB), so they show instantly across launches.
- Episode details show previous/next episode links and a "More in Season N" strip; stepping between episodes replaces the page instead of stacking it.
- Series pages have a Play/Resume button for the next episode to watch, and show the unwatched count.

### Changed
- Episode pages show the episode's own still instead of the series poster.
- Notices disappear after 3 seconds.
- Smooth motion without SVP only applies to titles with SVP switched off, and is never applied when the setting is off.
- Release archive renamed to embyclientplus-<version>-linux-x86_64.tar.gz.
- libmpv is built without JavaScript, Lua, CD/DVD/Blu-ray, libarchive, caca, sixel, JACK, sndio and Rubber Band, which the app doesn't use, so the bundle has fewer dependencies.
- Search results keep the server's relevance order instead of A–Z.
- The chosen quality carries over to the next episode.
- Series and details pages share one header layout: fixed-height banner, poster, title, details, buttons.

### Fixed
- Trailer items did nothing when opened.
- The More in Season strip sometimes started at the first episode or with a card cut off after moving between episodes; it now centres the current episode once laid out.
- Genre/tag tiles flashed a random cover mid-crossfade.
- Doubled, overlapping subtitles on transcoded quality presets: Emby burned the subtitle into the video even when asked not to; the burn-in parameters are now stripped from the transcode URL.
- Crash (abort) when changing quality or jumping to the previous/next episode from the player.
- Closing the window during playback now waits briefly so Emby gets the final position.
- Volume changes no longer send a report per step.
- Turning SVP off in the player didn't detach SVP Manager: mpv keeps already-connected IPC clients when its socket option is cleared. The app now also disconnects those clients, then removes SVP's filter.
- Banners and episode thumbnails no longer grow to the server image's size (layouts differed between series).

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
