# Emby Client+

A native Emby client for Linux, Windows and macOS with SVP frame interpolation, built for the desktop and Steam Game Mode.

## Description

A GTK4/libadwaita Emby client with libmpv embedded in-process. Its libmpv has VapourSynth, so
SVP 4 attaches just as it does to a standalone mpv.

## Features

- **Browsing:** Home rows (Continue Watching, Next Up, Latest, suggestions), library grids with tabs and remembered sort/filter, series and season pages, search across everything, cast & crew, More Like This, favourites, History page.
- **Playback:** direct play or transcode (Original down to 420 kbps); resume and watched state synced with Emby; audio, subtitle and quality choices remembered per title; chapters with seek-bar previews; Skip Intro/Credits; Up Next countdown; aspect and zoom; black-bar blur/glow; per-title volume.
- **Picture:** quality presets (Fast to High quality, or custom scalers and debanding), deinterlacing, software decoding; built-in FSR and Anime4K shaders plus your own, picked per title in the player.
- **SVP:** attaches automatically when SVP 4 is installed; per-title toggle; a status light on the SVP button; mpv frame blending as an alternative.
- **Downloads:** save movies and episodes in original quality and play them offline with the full player; progress syncs back when the server is reachable.
- **Music:** mini player and queue panel, shuffle/repeat, gapless, Emby playlists, Instant Mix, media keys (Linux).
- **Controller & Steam Deck:** everything works with a gamepad; on-screen button hints; every action remappable; fullscreen in Game Mode.
- **Standalone player:** `--player` plays local files and links with SVP and shaders, without Emby.
- **Other:** UI size 50–200%, light/dark theme, optional own `mpv.conf`, in-app YouTube trailers (with `yt-dlp`), remappable keyboard, keeps the screen awake during videos, self-updates.

## Installation

From [Releases](https://github.com/krakerz/EmbyClientPlus/releases):

| System | File | Install |
|---|---|---|
| Linux, SteamOS | `EmbyClientPlus-<version>-x86_64.AppImage` | `chmod +x`, then run it |
| Linux | `embyclientplus-<version>-linux-x86_64.tar.gz` | extract, run `./install.sh` (`./uninstall.sh` removes it); needs GTK 4.12+, libadwaita 1.6+, FFmpeg, libplacebo |
| Windows | `embyclientplus-<version>-windows-x86_64.zip` | extract anywhere, run `bin\embyclientplus.exe` |
| macOS (Apple Silicon) | `embyclientplus-<version>-macos-arm64.zip` | extract, move `Emby Client+.app` to Applications |

The macOS app isn't notarized. The first time it's blocked: open System Settings → Privacy &
Security → **Open Anyway**, or run `xattr -dr com.apple.quarantine "/Applications/Emby Client+.app"`.

Every build updates itself (Preferences → Updates). SVP 4 is optional and found in its usual
folder (`~/SVP4`, `C:\Program Files (x86)\SVP 4`, `/Applications/SVP 4 Mac.app`) or one you set.

## Building from source

Linux needs Rust, `meson`, `ninja` and the GTK4, libadwaita, FFmpeg, libplacebo, libass and
VapourSynth dev files:

```sh
scripts/build-libmpv.sh && cargo build --release && target/release/embyclientplus
EMBYCLIENTPLUS_PORTABLE=1 cargo build --release && packaging/package.sh   # release packages → dist/
```

Windows (MSYS2 UCRT64) and macOS (Homebrew) use their packaged `mpv`; see the `windows` and
`macos` jobs in `.github/workflows/build.yml` and `packaging/package-{windows,macos}.sh`.

## Usage

Start the app and sign in to your Emby server (remembered). Pick a title and press **Play** or
**Resume**. Settings are in Home's menu → **Preferences**.

**Downloads:** choose **Download** in a movie's or episode's menu (X on a controller,
right-click with a mouse). Files go to `Videos/Emby Client+` (Preferences → Downloads to change
it), and **Downloads** in Home's menu lists them. Downloaded titles always play from disk.

**Server identity:** by default the server lists the app as a web browser (Emby Web). Turn off
Preferences → Server → "Appear as a web browser" to list it as Emby Client+ under your computer's
name; sign in again for the server to pick it up.

### Video quality and shaders

Preferences → Video sets the picture preset: **Auto** (mpv defaults or your mpv.conf), **Fast**
(Steam Deck), **Balanced**, **High quality**, or **Custom**. Heavier settings cost more GPU, more
again with SVP.

Shaders come in groups: **FSR** (Sharp, Balanced, Soft), **Anime4K** (modes A–C+A, Fast and HQ),
and your own. The player's **Shaders** button picks one preset per group, remembered per title
(per series for episodes); groups stack in the order Anime4K, yours, FSR. Upscalers only help when
the video is smaller than the screen.

Your own shaders go in the `shaders` folder next to `config.toml` (Preferences → Video → Open
Folder): each folder is a group, each `.glsl` file a preset, and each subfolder a preset of
several `.glsl` files applied in name order.

### Keyboard

| Key | Action | Key | Action |
|---|---|---|---|
| Space / K | Play/pause | , / . | Previous/next episode |
| ← / → | Seek ±10 s | PgUp / PgDn | Previous/next chapter |
| ↑ / ↓ | Volume ±5 | M | Mute |
| F / F11 / double-click | Fullscreen | Esc | Leave fullscreen, then the player |
| Q | Skip intro/credits | | |

Change them in Preferences → Keyboard.

### Controller

The on-screen legend (bottom left) always shows the current screen's buttons. Defaults, all
remappable per context in Preferences → Controller:

| Button | Browsing | Player | Music panel |
|---|---|---|---|
| A | Select | Play/pause | Select |
| B | Back | Leave player | Back |
| X | Item menu | Audio menu | Pick up queue item to move |
| Y | Home | Subtitle menu | Play/pause |
| LB / RB | Previous/next tab or season | Previous/next episode | Previous/next track |
| LT / RT | Open music panel | Previous/next chapter | Close panel |
| D-pad / stick | Move | Seek (←/→), volume (↑/↓) | Move (a picked-up item) |
| Select | Search | Skip intro/credits | |
| Start | Preferences | On-screen controls | |

On settings rows, Left/Right change the value and A cycles options.

### Steam Game Mode

Add the AppImage (or `~/.local/bin/embyclientplus` from the archive) as a non-Steam game with the
**Gamepad** controller layout. It runs fullscreen there (Preferences → Display → Fullscreen).

### Command line

```sh
embyclientplus                       # the app
embyclientplus --player [file|url]   # just the player; plays the next video in the folder after it
embyclientplus <file|url>            # same as --player <file|url>
embyclientplus --help | --version
```

## FAQ

**SVP doesn't kick in?** The SVP button shows why: green interpolating, amber waiting for SVP
Manager, red SVP Manager not running, grey SVP not found.

**Where are settings and logs?** `config.toml` and `logs/` (newest 10 kept) are in
`~/.config/embyclientplus/` (Linux), `%APPDATA%\krakerz\embyclientplus\config\` (Windows) or
`~/Library/Application Support/com.krakerz.embyclientplus/` (macOS).

**Does it bundle SVP?** No, it uses your own SVP install.

**Is HDR supported?** No. HDR videos play converted to SDR colours, untested for lack of HDR
content or a display. SVP starts off for HDR titles unless you turn it on for that title.

---

### Notes

Icons: [Lucide](https://lucide.dev/) (ISC, `data/icons/LICENSE`), converted for GTK by
`scripts/update-icons.sh` (needs `picosvg`).

Built and maintained with the help of AI.
