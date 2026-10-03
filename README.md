# Emby Client+

A native Linux Emby client with SVP frame interpolation, built for the desktop and Steam Game Mode.

## Description

A GTK4/libadwaita Emby client with libmpv embedded in-process. Its libmpv is built with
VapourSynth, so SVP 4 attaches just as it does to a standalone mpv.

## Features

**Browsing:**
- Home page with Continue Watching, Next Up, Latest per library and TV Suggestions ("Because you watched…").
- Library grids with tabs (Movies/Shows, Genres, Tags, Collections, Folders, Favorites); sort, order and filter remembered per library.
- Series and episode pages with season selector, unwatched counts, and Resume/Play for the next episode.
- Search across all libraries (Movies, Shows, Episodes, People, Collections, Albums, Songs, Artists) with tabbed results.
- Cast & Crew row (links to actor/director work); "More Like This" row on details and series pages.
- Mark watched/unwatched/favourite from card context menu or details page.
- Favorites split by kind (Movies, Shows, Episodes, Albums, Songs, Artists).
- Genre and tag tiles with random cover cycling on hover/focus.
- Music libraries: Albums, Artists, Songs, Latest, Favorites, Folders tabs; album pages list tracks with disc/track numbers.

**Playback:**
- Direct play or Emby transcode: Original, or a bitrate cap from 40 Mbps down to 420 kbps.
- Resume position and watched state sync with Emby; previous playback choices (audio/subtitle/quality) remembered per title.
- Chapters and chapter markers on the seek bar; previous/next chapter navigation.
- Chapter and seek-bar preview (frame, chapter name and time) on hover, drag or controller.
- Audio and subtitle track pickers with preferred language defaults.
- Skip Intro/Credits buttons (from Emby markers).
- Up Next countdown (10 s) when credits begin; can autoplay the next episode or cancel.
- Previous/next episode buttons (for series).
- Fullscreen with auto-hiding OSD (hides after 3 s without pointer movement while playing).
- Aspect ratio (Fit, Fill, Stretch, 16:9, 4:3, 21:9) and zoom per title.
- Black-bar fill on the sides/top (Off, blur or edge glow) with subtitles inside the picture.
- Volume control and mute; on-screen volume indicator.
- Per-title SVP toggle with global default; smooth motion without SVP (mpv frame blending).
- Picture quality presets (Auto, Fast, Balanced, High quality, Custom scalers and debanding), deinterlacing, optional software decoding.
- Shaders: built-in AMD FSR and Anime4K, plus your own; pick per title from the player, stack groups together.

**Music:**
- Compact floating mini player at the bottom right; full-height music panel opens upward with Now Playing and queue.
- Shuffle, repeat (off/all/one), gapless playback.
- Emby playlists: browse, play, shuffle, reorder, remove tracks, create new from Add-to-Playlist dialog.
- Queue reordering: drag rows with mouse, or move with controller (X button, then Up/Down, then A/B/X to drop).
- Play Now, Play Next, Add to Queue, Add to Playlist, Instant Mix on albums, artists and individual tracks.
- Desktop media controls and media keys (MPRIS).
- Album cover, artist and album links.
- Music does not use SVP; dedicated music section isolated from video playback.

**SVP / Frame Generation:**
- Per-title smooth motion toggle; global default can be on or off.
- SVP 4 auto-attachment when installed (`~/SVP4` or custom folder).
- SVP status light: green (interpolating), amber (waiting for SVP Manager), red (not running), grey (not found).
- In Steam Game Mode, the app starts SVP Manager itself if SVP is enabled.
- HDR titles start with SVP off unless you turn it on for that title.

**Controller & Steam Deck:**
- Navigate and play everything with a gamepad (D-pad/left stick, buttons, triggers).
- On-screen button legend at the bottom left, showing your current mapping; hidden in the player and with mouse/touch.
- Every action remappable in Preferences → Controller (separate sections for Browsing, Player, Music).
- Left/Right on settings rows change the value (combo boxes, spin rows, sliders, switches); A cycles through combo options.
- Focus stays on the same item when switching tabs or episodes, or after list reloads.
- Steam Game Mode: add the AppImage (or `~/.local/bin/embyclientplus` if installed) as a non-Steam game with the Gamepad layout.
- Fullscreen setting: Gamescope only (Steam Game Mode), Always, or Never; the player never drops the app out of it.
- Start button in the player brings up the OSD's buttons in a navigable mode (D-pad/stick to move, A to press, B or Start to leave).

**Other:**
- Emby login and auto-login (token stored securely in system keyring with plaintext fallback).
- Theme: Light, Dark, or Follow system.
- Optional: load your own `~/.config/mpv/mpv.conf` (shaders, scalers, subtitle styling; app-managed options are ignored).
- In-app trailers: YouTube trailers play natively when `yt-dlp` is installed, otherwise open in the browser.
- Trailers with configurable quality (best available or capped at 4K/1440p/1080p/720p) and captions in your subtitle language.
- Log files: newest 10 stored at `~/.config/embyclientplus/logs/`.
- Poster cache (512 MB) for instant loading across launches.
- Self-updates from GitHub releases (Preferences → Updates).
- Quit button in Preferences header (handy in Steam Game Mode).
- Keeps the screen on while a video plays (not for music or paused videos); can be turned off in Preferences.

## Installation

From [Releases](https://github.com/krakerz/EmbyClientPlus/releases):

- **AppImage** (recommended, works on SteamOS): `chmod +x`, then run it.
- **Archive** `embyclientplus-<version>-linux-x86_64.tar.gz`: extract it and run `./install.sh`.
  - `./uninstall.sh` removes it and keeps your settings.
  - It needs GTK 4.12+, libadwaita 1.6+, FFmpeg and libplacebo from your distro.

Both formats update themselves (Preferences → Updates). SVP 4 is optional, found in
`~/SVP4` or a folder you set; the app plays normally without it.

## Building from source

You need Rust, `meson` and `ninja`, plus the GTK4, libadwaita, FFmpeg, libplacebo, libass
and VapourSynth dev files.

```sh
scripts/build-libmpv.sh && cargo build --release && target/release/embyclientplus
EMBYCLIENTPLUS_PORTABLE=1 cargo build --release && packaging/package.sh   # release packages → dist/
```

## Usage

1. Start `embyclientplus` and sign in to your Emby server. The login is remembered.
2. Browse your library and pick a title; press **Play** or **Resume** to start playback.

Settings are in Home's menu → **Preferences**: SVP, quality, languages, display, artwork,
mpv.conf, controller remapping, updates and logs.

### Video quality and shaders

**Preferences → Video** sets the picture quality for every video:

- **Auto** keeps mpv's defaults (or your mpv.conf, if "Use my mpv.conf" is on).
- **Fast** suits the Steam Deck; **Balanced** is a middle ground; **High quality** uses mpv's sharpest scalers plus debanding.
- **Custom** lets you choose the upscaler, chroma scaler, downscaler and debanding yourself.

Heavier scalers and shaders cost more GPU, and more again with SVP, since every interpolated frame is processed too.

**Shaders** are grouped. Built in: **FSR** (AMD FidelityFX Super Resolution: Sharp, Balanced, Soft) and **Anime4K** (modes A/B/C/A+A/B+B/C+A, each in a Fast variant for weaker GPUs like the Steam Deck and an HQ variant for desktop GPUs). In Preferences → Video, each group can be shown or hidden in the player and given a default preset (Off unless you pick one).

In the player, the **Shaders** button has a submenu per shown group: pick Off or one preset. The pick is remembered for that title (per series for episodes). Groups stack, so for example Anime4K Mode A and one of your own presets can run together; they apply in this order: Anime4K, your groups, then FSR. Upscaling shaders only do something when the video is smaller than the screen (e.g. 720p on a 1080p display; on the Steam Deck's 800p screen, 1080p video is shrunk, so they have no effect).

**Your own shaders** go in `~/.config/embyclientplus/shaders/` (Preferences → Video → Your shaders → Open Folder). Each folder is a group; each `.glsl` file in it is a preset, and each subfolder is a preset whose `.glsl` files are applied in name order:

```
shaders/
  MyUpscale/          → group "MyUpscale"
    Sharp.glsl        → preset "Sharp"
    Soft/             → preset "Soft"
      1-denoise.glsl
      2-upscale.glsl
```

New folders show up the next time Preferences opens or a video starts.

### Keyboard

| Key | Action | Key | Action |
|---|---|---|---|
| Space / K | Play/pause | N / P | Next/previous episode |
| ← / → | Seek ±10 s | PgDn / PgUp | Next/previous chapter |
| ↑ / ↓ | Volume ±5 | M | Mute |
| F / F11 | Fullscreen | Esc | Leave fullscreen (then back) |
| Double-click | Fullscreen | | |

### Controller

The on-screen button legend (bottom left) always shows what each button does on the current screen.
Every action is remappable in Preferences → Controller. Below are the defaults.

**Browsing & Navigation:**

| Button | Action | Button | Action |
|---|---|---|---|
| D-pad / L-stick | Move up/down/left/right | A | Select / Activate |
| B | Back / Close | Y | Home |
| X | Item menu / Options | LB | Previous tab / season |
| RB | Next tab / season | Select | Search |
| Start | Preferences | LT / RT | Open / close music panel |

**In Settings Rows:**
- Left / Right: Change value (combo boxes step through options, spin rows and sliders step, switches toggle)
- A: Cycle to next option (combo rows only)

**Player:**

| Button | Action | Button | Action |
|---|---|---|---|
| A | Play / pause | D-pad / L-stick | Seek (←/→) or volume (↑/↓) |
| LB | Previous episode | RB | Next episode |
| LT | Previous chapter | RT | Next chapter |
| X | Audio menu | Y | Subtitle menu |
| Select | Skip intro/credits | Start | Show on-screen controls (navigate with D-pad, A to press, B to close) |
| B | Leave player | | |

**Music Panel:**

| Button | Action | Button | Action |
|---|---|---|---|
| Y | Play / pause | LB | Previous track |
| RB | Next track | X | Pick up queue item to move |
| LT / RT | Close / back to library | D-pad ↑/↓ | Move queue item up/down (when picked up) |
| A / B / X | Drop queue item (when picked up) | | |

Button remapping is per-context (Browsing, Player, Music), so the same button can do different things
depending on where you are. The on-screen hints automatically follow your mapping.

### Steam Game Mode

Add the AppImage (or the installed `~/.local/bin/embyclientplus` if you used the archive installer)
as a non-Steam game to Steam, and set the controller layout to **Gamepad**. The app runs fullscreen
in Game Mode (Preferences → Display → Fullscreen).

### Command Line

```sh
embyclientplus                 # Start the app
embyclientplus <file-or-url>   # Play a file or stream without Emby (for testing player or SVP)
embyclientplus --help          # List options
embyclientplus --version       # Print the version
```

## FAQ

**SVP doesn't kick in?** The SVP button shows the state:

- green: interpolating;
- amber: waiting for SVP Manager;
- red: SVP Manager isn't running;
- grey: SVP not found.

**Where are settings and logs?** In `~/.config/embyclientplus/`: `config.toml` and `logs/`
(the newest 10 are kept).

**Does it bundle SVP?** No. It uses your own SVP install.

**Is HDR supported?** No. HDR videos play, but they're converted to normal (SDR) colours; there's no HDR output, and HDR hasn't been tested since the developer has no HDR content or display. SVP stays off for HDR titles unless you turn it on for that title.

---

### Notes

Icons: [Lucide](https://lucide.dev/) (ISC, `data/icons/LICENSE`), converted for GTK by
`scripts/update-icons.sh` (needs `picosvg`).

Built and maintained with the help of AI.
