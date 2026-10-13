# Changelog

## [Unreleased]

## [1.16.1] — 2026-10-13

### Added
- Preferences → Home → "In Progress" filter for TV (off by default): turn it on to get "In Progress" in TV libraries' filters; it's off by default because finding the started series can take a moment with a long watch history.

### Fixed
- With a controller, Up from the top row of a library grid didn't reach the bar above it (filter, sort), so the filter couldn't be changed: since the bar became a centred group, it wasn't straight above most cards. Up from the top row now always goes to the bar.
- With a controller, the grid's filter and sort lists opened but their choices ignored A, so nothing could be picked. A on the filter or sort now steps to its next choice (wrapping), as it already did on Preferences' choice rows; a mouse still opens the list.
- With a controller, A on a row of the Downloads page now does what its button does: pauses a running download, resumes a paused one (the buttons at the row's end still work too).
- Steam Game Mode: Profile no longer offers "Change Picture…", whose file chooser can't be used with a controller there (the same as Preferences' folder picker).
- The "In Progress" filter never showed anything in TV libraries: Emby doesn't mark a series itself as in progress, only its episodes. It now lists the series you've started (an episode watched or part-watched) and haven't finished.

## [1.16.0] — 2026-10-13

### Added
- Downloads survive closing the app, even an abrupt close or crash: what was downloading or waiting is remembered, and a download that was cut off continues from where it stopped instead of starting over. Preferences → Downloads → "Resume downloads at start" (on by default) carries them on when the app opens again; off, they wait on the Downloads page.
- The Downloads page can pause and resume: a running download has Pause (keeps what's downloaded) and Cancel (deletes the partial file); paused ones are listed under "Paused" with Resume, and "Resume All". A download interrupted by a lost connection lands there too, ready to continue.
- The Downloads page shows how much space downloads use and how much is free on that disk, and groups finished downloads by series, each with its total size.
- Preferences → Playback → "Quality menu for downloads", off by default: when on, downloaded titles keep the Quality button, and picking a lower quality streams the title from the server from the same spot; Original goes back to the download. If the server can't be reached, it plays the download instead.
- The player says when a video plays from this device: a "Downloaded" tag (or "Local file" in the standalone player) at the right of the top bar, appearing with the controls.

### Changed
- Downloads check for disk space first and keep at least 1 GB free: when there isn't room, the download and everything waiting pause with a note saying how much space is needed, instead of filling the disk. Auto-download leaves paused episodes alone.
- Library grids (and search, playlists, artists, collections, More Libraries) are packed with the same tight 12px gap as Home's rows and centred, fitting as many columns as the window allows; a short list (even a single result) sits in the middle, and the bar above (item count, filter, sort) sits centred over them. Genre and tag tiles are centred too.

### Fixed
- A downloaded title showed the Quality button, and picking a quality quietly switched to streaming it from the server. Downloads now hide it, like other local files.
- Playing a downloaded episode, Up Next and previous/next only knew the other downloaded episodes: with episodes 2 and 4 downloaded but not 3, episode 2 went straight to 4. When the server is reachable they now follow the series itself, playing the next episode from disk if it's downloaded and streaming it if not; offline, the downloaded episodes still follow each other.
- mpv no longer logs "vf-remove: item label @svp not found" before every video when SVP isn't attached.

## [1.15.0] — 2026-10-13

### Added
- Auto-download for series: "Auto-download…" in a series' Download menu (or its card's menu) keeps its next unwatched episodes downloaded, 1 to 20 of them, as the usual original files, and can delete episodes once they're watched. It checks when the app starts, every half hour, and shortly after you stop watching something; the Downloads page lists each series' setting to change or stop it.
- Downloaded titles are marked: a small download badge on their artwork in episode lists and on movie and episode cards. Movie and episode pages have a download button beside the favourite star: it downloads the title, shows a spinner while it's queued or downloading (opening Downloads), and turns green once downloaded, offering to delete the download.
- Steam library artwork (cover, wide cover, background and logo) for adding the app as a non-Steam game: in the Linux archive's `steam-library` folder, and as its own download on the release page.

### Changed
- Every dialog now looks like Preferences and Profile, in the app's colours with a close button at the top: Add to Playlist, Open URL, the SVP folder, and the sign-in-again prompt no longer use the plain grey pop-up style.

## [1.14.1] — 2026-10-13

### Added
- A "Show More Libraries" switch in Preferences, under Home, off by default. Home's "More Libraries" card (for libraries hidden from Home) now appears only when it's on.

### Fixed
- Profile: your picture and name now sit at the top, followed by Change Password and Profile PIN, then the settings; before, the picture appeared below the password rows. Sign Out is gone from Profile (it's "Log Out" in Home's menu), and so is the menu's Profile item, which duplicated the avatar button.
- Profile's "Hide watched items in Suggestions" had no effect on TV libraries: their Suggestions tab always hid watched series. It now follows the switch, as movie libraries already did.

### Removed
- Profile's "Show missing episodes" switch, which had no effect in this app.

## [1.14.0] — 2026-10-13

### Added
- Profile: your own Emby account settings, from the avatar at the top right of Home (or the menu's Profile). Change your picture or password, set a profile PIN, sign out, and edit the settings Emby's own apps share: audio and subtitle language, subtitle mode (including Smart, Only forced and None), playing the default audio track, remembering audio and subtitle choices, autoplaying the next episode, rewinding on resume, skipping intros (button, automatic or not at all), missing episodes, hiding watched items, and each library's place on Home, with one switch to show or hide it there (its Latest row included). Changes are saved to the server as you make them.
- Libraries hidden from Home stay reachable: a "More Libraries" card at the end of Home's Libraries row lists them.
- Player-only mode has its own copies of these playback settings in Preferences, saved in the app's config, so playing files never needs a server.

### Changed
- Signed in, playback follows your Emby account's settings instead of the app's: Preferences no longer shows its own audio and subtitle languages then (they're on Profile). A copy of the account's settings is kept for playing downloads without a connection.

## [1.13.1] — 2026-10-11

### Fixed
- The Trailers library's tile on Home showed a cinema photo instead of the "Trailers" art Emby's own apps show. Library tiles now use the same picture as Emby.
- Home ignored your Emby display settings for libraries: it showed libraries you've hidden from My Media (Playlists, say), in the server's default order, with "Latest" rows for libraries you've excluded from them. It now follows those settings, as Emby's own apps do.
- Round pictures (artists in Favorites and the music library, people in Cast & Crew and search) could be stretched into ovals: with a UI scale and a high-DPI display, wide images (logos, band photos) were given more room than portrait ones. Every picture is now exactly its card's size, so circles stay circles, and posters and other cards no longer vary in width with their image.

## [1.13.0] — 2026-10-11

### Added
- TV libraries get a Collections tab too, shown only when the library has collections (an Anime library with some does, a TV Series library without any doesn't). Movie libraries now hide theirs when empty, the same way.
- A "Version" button on movie and episode pages for titles with more than one version (a 4K and a 1080p file, say), next to Subtitles. "Default" leaves the choice to Emby; a chosen version applies until the app is closed.
- Windows and macOS: media keys and the system's "now playing" controls work with the music player (play/pause, next, previous, seeking, with the track's title, artist and cover), as they already did on Linux.
- Movie and series pages show "Included In": the collections the title belongs to, between Cast & Crew and More Like This, like Emby's own apps.

### Fixed
- Albums without a cover of their own showed a music note, where Emby's apps show the cover embedded in their tracks. The app now uses the same borrowed cover.

## [1.12.3] — 2026-10-11

### Fixed
- Leaving the player (back to the library) now disconnects SVP Manager too, not just closing the app. It stayed attached to the idle player before, waiting for a video that wasn't coming, which could leave the next playback's SVP stuck yellow.
- macOS: turning SVP off in the player (and now leaving the player or closing the app) didn't disconnect SVP Manager, which stayed attached until the next video. It's disconnected straight away there too, as on Linux and Windows.
- Windows and macOS: "Keep the screen on" didn't work, so the display could dim or sleep mid-video. Both now ask the system directly while a video plays, as Linux already did through the desktop.
- macOS: hardware decoding asked for VA-API (Linux only) before falling back to VideoToolbox; it now asks for VideoToolbox directly.

## [1.12.2] — 2026-10-11

### Fixed
- Closing the app now disconnects SVP Manager straight away. Before, its connection lingered until the app's process had fully exited, which could leave SVP stuck "waiting for the video info" (yellow) when the app was opened again.

## [1.12.1] — 2026-10-11

### Fixed
- SVP started twice on each new episode: the previous episode's interpolation carried over into the next one until SVP Manager replaced it a few seconds later. A new video now starts clean, and SVP attaches once.

## [1.12.0] — 2026-10-11

### Added
- Preferences → General → Playback: "Up Next countdown" (3–60 seconds, 10 by default) and "Up Next opacity" (20–100%), so the card covers less of the picture.

### Changed
- Up Next now counts down to the episode's very end instead of starting at the credits, so scenes after the credits aren't cut off. With a 10-second countdown the card appears 11 seconds before the end and the next episode starts when this one finishes. The count follows the video, so pausing holds it.

## [1.11.1] — 2026-10-10

### Fixed
- The player buttons' tooltips named the wrong keys ("Previous episode (P)" while the key is ","). They now show the key you've actually mapped in Preferences → Keyboard, and no key at all while you're using a controller.
- The SVP button always said "~/SVP4 not found" when SVP was missing, whatever the system or the folder set in Preferences. It now names the folder it actually looked in.

## [1.11.0] — 2026-10-09

### Added
- macOS: the app menu works: About Emby Client+, Settings… (⌘,) and Quit (⌘Q). Ctrl+, opens Preferences and Ctrl+Q quits on Linux and Windows.

### Fixed
- Quitting with more than one dialog open (About over Settings, say) left the app running.
- macOS: SVP wasn't found ("SVPManager not found here"). The app now looks inside `SVP 4 Mac.app` correctly, accepts either the app or the folder it's in (Applications) as the SVP folder, and finds Homebrew's or MacPorts' VapourSynth the way SVP does.

## [1.10.1] — 2026-10-09

### Fixed
- As a web browser (the default), the server's Devices page showed the app's version ("Emby Web 1.10.0") instead of the server's, like Emby's own web app. The app now asks the server for its version every time it starts.

## [1.10.0] — 2026-10-09

### Added
- Subtitles button next to Play on movie, episode and series pages: pick a language (or forced only, or off) before playing; it's remembered for the title (the whole series for episodes), and Default goes back to Preferences.
- Download a whole series or season: from its menu, or the download button on the series page. Downloads now run one at a time; the rest wait on the Downloads page, where each can be cancelled.
- Downloads save their subtitle files too, and they load when the title plays from disk. The standalone player also loads subtitle files named like the video (`Movie.eng.srt`).
- Player: the subtitles menu has Size (− / + in 10% steps, kept for every video) and Timing (± 0.1 s, for the current video; plus shows them later); click the value to reset it.
- Linux: Preferences → Playback → "Do not disturb while watching" holds notifications back while a video is open (KDE Plasma, GNOME, caelestia, swaync, dunst, mako, or your own on/off commands).

### Changed
- Changing the volume or muting (keyboard or controller) shows just the volume indicator, not the whole player controls.

## [1.9.0] — 2026-10-09

### Added
- The player's top bar (title and back button) works as a title bar: drag it to move the window, double-click it to maximize.

### Fixed
- Windows: turning SVP off and on again during playback broke it ("Couldn't create first pipe instance"); SVP is now disconnected cleanly, as on Linux.
- Windows: SVP couldn't interpolate ("Could not initialize VapourSynth scripting"): the player couldn't reach SVP's VapourSynth and its Python. VapourSynth now writes its own log, `vapoursynth.log`, next to the app's logs.

## [1.8.0] — 2026-10-09

### Added
- Windows: `Emby Client+.exe` at the top of the folder starts the app; both it and the app's exe have the app icon.

### Changed
- Windows and macOS keep settings and logs in `%APPDATA%\EmbyClient+` and `~/Library/Application Support/EmbyClient+` (moved over on first start), and caches in `%LOCALAPPDATA%\EmbyClient+\cache` and `~/Library/Caches/EmbyClient+`.
- Windows: SVP is found in `C:\Program Files\SVP 4` (and the older `Program Files (x86)` location).
- Windows: hardware decoding uses D3D11 first.

### Fixed
- Windows: SVP couldn't interpolate ("Could not initialize VapourSynth scripting"): SVP's VapourSynth couldn't find its Python. Its log, `vapoursynth.log`, sits next to the app's logs.

## [1.7.0] — 2026-10-09

### Added
- Windows (x86_64) and macOS (Apple Silicon) builds, with SVP support through SVP's own VapourSynth, and self-update like the Linux builds.
- Settings, cache and downloads use each system's usual folders; the user's mpv.conf is read from `%APPDATA%\mpv` on Windows and `~/.config/mpv` elsewhere.

### Changed
- The app now identifies itself to the server as a web browser (Emby Web, with the server's version) by default; Preferences → General → Server → "Appear as a web browser" turns that off, and the server then lists it as Emby Client+ under this computer's name. Switching offers to sign out, since the server only picks up the new name at sign-in.

## [1.6.0] — 2026-10-08

### Added
- Keyboard: Q skips the intro or credits (remappable in Preferences → Keyboard).

### Changed
- Keyboard: previous/next episode are now , and . by default (were P and N).
- Quit moved from the Preferences header to Home's menu, under Log Out.

## [1.5.0] — 2026-10-05

### Added
- Downloads for offline playback: Download in a movie's or episode's menu saves the original file (with its artwork and details) to a folder you choose in Preferences; the Downloads page (Home's menu) lists them, shows progress and deletes them.
- Downloaded titles play from disk, with SVP and shaders as usual; the next downloaded episode plays after each one, and progress still syncs to Emby when the server is reachable.
- When the server can't be reached, Home says so, with Try Again and a shortcut to your downloads.
- History page (Home's menu): everything you played, newest first, grouped by day.
- Standalone player: `embyclientplus --player [file|url]` opens just the player (SVP, shaders, picture options, tracks), and plays the next video in the folder after each one.
- Without a file, the standalone player starts on a page where you can drop files or links, open a file or URL, or pick a video from your downloads folder; its Preferences leave out the server settings.
- Preferences → Keyboard: change the player's keyboard shortcuts.
- Each title remembers its own volume (including boost above 100%); Preferences → Playback sets the default.
- Preferences → Playback: option to hide the player controls while paused too.
- Episode rows and details show when an episode aired and when it was added.

### Fixed
- Rows and grids with only a few items no longer spread their cards across the whole width.
- The Fast, Balanced, High quality and Custom picture presets failed to apply some settings (deinterlacing, decoding).
- The Preferences tabs no longer cut off "Controller".

## [1.4.0] — 2026-10-03

### Added
- UI size buttons (reset, −, +) next to Home's menu scale text and artwork from 50% to 200%; the size is remembered.
- Series and season cards show how many episodes are still unwatched, at the end of the line under the title.

### Changed
- The app version now shows next to the UI size buttons on Home, instead of in the Preferences header.
- The watched mark moved off the artwork to the end of the line under the title, as a green check in a circle.

### Fixed
- Keyboard: Space and the arrow keys in the player sometimes did nothing until the player was clicked.

## [1.3.0] — 2026-10-03

### Added
- Preferences → Video: picture quality presets (Auto, Fast, Balanced, High quality, Custom), with upscaler, chroma scaler, downscaler and debanding choices for Custom.
- Preferences → Video: deinterlacing for interlaced videos (on by default) and a software decoding switch.
- Built-in shaders: AMD FSR (Sharp, Balanced, Soft) and Anime4K (modes A, B, C, A+A, B+B, C+A, each Fast or HQ).
- Your own shaders: each folder in `~/.config/embyclientplus/shaders/` becomes a shader group with its presets.
- Player: a Shaders button with a submenu per shader group; each group's pick is remembered per title, and groups can be combined.
- Preferences → Video: show or hide each shader group and choose its default preset.
- Player: the Shaders button turns green while any shader is on, and its menu shows a check next to each group that has a preset on.

### Changed
- Player: the SVP button shows its state by colour only, without a pressed background.
- Player: all buttons, including Back, are now the same round shape, and the control bar is more compact.
- Player: the SVP button uses an icon, so it sits centred in its button.

### Fixed
- Controller: moving up or down past the edge of an open dialog no longer jumps the cursor onto the page behind it.
- Controller: buttons pressed while the Shaders menu is open no longer act on the player behind it.

## [1.2.0] — 2026-10-03

### Added
- Preferences → Playback: "Keep the screen on" stops the screen blanking and the system sleeping while a video plays (on by default; music and paused videos don't count).

### Changed
- Preferences → Display: the fullscreen setting now applies everywhere and right away. "Gamescope only" (the old Auto) keeps the app fullscreen in Steam Game Mode; "Always" keeps it fullscreen on the desktop too, and leaving the player no longer drops it out of fullscreen.

### Fixed
- Controller: LB/RB on tabbed pages (library tabs, Search, Preferences) no longer nudge the cursor sideways to a neighbouring item after switching tabs.
- Controller: Y (Home), View (Search) and the music button no longer act on the page behind an open dialog; with an item menu open, buttons that don't navigate it close it instead.
- Genre and tag tiles show a single focus ring around the cover instead of two.
- Series pages with many seasons: LB/RB now scroll the season row so the selected season stays on screen.

## [1.1.1] — 2026-10-03

### Fixed
- Controller: Down got stuck on the first row of the music queue (and other scrolling text in the music panel and legend could catch the cursor).

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
