# Emby Client+

A native Emby client for Linux, Windows and macOS with SVP frame interpolation, built for the desktop and Steam Game Mode.

## Description

Emby's official desktop apps either wrap a web view or lack VapourSynth support for SVP 4 frame interpolation and controller-first Steam Game Mode integration. Emby Client+ pairs a native GTK4/libadwaita interface with an in-process `libmpv` built with VapourSynth, so SVP 4 attaches just as it does to standalone `mpv`, alongside full gamepad navigation and automatic display refresh-rate switching in Game Mode.

## Features

- **Browsing:** Home follows your Emby library order and visibility; library grids with remembered sort and filters (decade, HD/SD, subtitles, rating, recently added) and an A–Z strip, Suggestions, Episodes (TV) and Collections tabs; series open on the season you're watching; details show original title, air dates, studios, genres, tags and each season's own overview when the server has them; cast, Extras and Parts, More Like This, Included In, favourites, History, search.
- **Playback:** direct play or transcode (Original down to 420 kbps), version picker, resume and watched state synced, chapters with seek-bar previews, Skip Intro/Credits (or always skip/always show per series), Up Next, sleep timer, aspect/zoom, black-bar blur/glow.
- **Picture:** quality presets or custom scalers and debanding strength, deinterlacing, and per-title shaders: FSR, Anime4K, chroma upscalers (KrigBilateral, Chroma from Luma), luma upscalers (RAVU, NNEDI3, FSRCNNX), sharpeners (AMD CAS, Adaptive sharpen), plus your own.
- **SVP:** attaches when SVP 4 is installed; per-title toggle with a status light; mpv frame blending as an alternative; Preferences shows SVP's main settings and can override them.
- **Profile:** your Emby account's own settings (languages, subtitle mode, autoplay, intro skipping, hidden watched items, library order), picture, password and PIN; shared with Emby's apps. Audio and subtitle languages can also be set per library (Preferences → General → Library languages), and each series or movie remembers its own.
- **Tracking:** MyAnimeList, AniList, Kitsu and Trakt follow what you watch, including split anime seasons (matched through AniBridge) and downloads watched offline; MyAnimeList and AniList log in from Desktop Mode; Trakt uses your own free Client ID (see [below](#trakt-use-your-own-client-id)).
- **Downloads:** original files or smaller server-converted copies (1080p/720p; default, per series or asked each time) for offline play with the full player; pause and resume (also all at once), clear the queue, resuming after the app closes, with disk space shown; auto-download keeps a series' next unwatched episodes and can delete watched ones; downloaded titles are badged.
- **Music:** mini player, queue, shuffle/repeat, gapless, playlists, Instant Mix, media keys and now-playing on every OS.
- **Controller & Steam Deck:** everything works with a gamepad, with on-screen hints and remappable buttons; Steam library artwork included.
- **Also:** standalone player for local files and links (`--player`), UI size 50–200%, light/dark theme, own `mpv.conf`, YouTube trailers (`yt-dlp`), self-updates.

## Dependencies

### Runtime

- **Pre-built releases:**
  - **Linux AppImage (`.AppImage` / `-compat.AppImage`):** bundles GTK4, libadwaita, `libmpv`, FFmpeg and `libplacebo`; needs standard host desktop graphics (OpenGL/EGL/Vulkan) and audio libraries. The plain AppImage uses the host's `glibc`; the `-compat.AppImage` bundles its own `glibc` (SVP inside it still requires host `glibc` ≥ 2.38).
  - **Linux archive (`.tar.gz`):** bundles `libmpv`, FFmpeg and `libplacebo`, and requires system **GTK ≥ 4.12** (`gtk4`), **libadwaita ≥ 1.6** (`libadwaita-1`) and a current `glibc`.
  - **Windows / macOS (`.zip`):** self-contained; all required runtime libraries are bundled.
- **Optional (all platforms):**
  - **Emby Server:** required for browsing and streaming your library (not needed for standalone `--player` playback).
  - **SVP 4:** optional, for SVP frame interpolation. On macOS, SVP also requires VapourSynth (`brew install vapoursynth`).
  - **`yt-dlp`:** optional, on `PATH`, for playing YouTube trailers and web video page links.

### Build

- **All platforms:**
  - **Rust ≥ 1.85** (Rust 2024 edition) and Cargo
  - C compiler (`cc` / `gcc`) and `pkg-config` (`pkgconf`)
  - **GTK ≥ 4.12** (`gtk4`) and **libadwaita ≥ 1.6** (`libadwaita-1`) development files
- **Linux** (builds a private VapourSynth-enabled `libmpv` `v0.42.0` via `scripts/build-libmpv.sh`, because distro `libmpv` packages ship without VapourSynth):
  - Extra tools: `git`, `meson`, `ninja` (`patchelf` if running `packaging/package.sh`)
  - Libraries/headers (`pkg-config` / library names): `ffmpeg`, `libplacebo`, `libass`, `vapoursynth` (including `VSScript4.h`, `libvapoursynth.so`, `libvapoursynth-script.so`), `zimg`, `lcms2`, `uchardet`, `wayland`, `wayland-protocols`, `xkbcommon`, `libdrm`, Mesa/OpenGL/EGL, Vulkan headers & ICD loader, `libva`, `libvdpau`, `libdisplay-info`, X11 extensions (`libXpresent`, `libXss`, `libXv`, `libXrandr`), PipeWire / PulseAudio / ALSA, and `libsystemd`
  - **Arch Linux:**
    ```sh
    sudo pacman -Syu --needed \
      base-devel git rust meson ninja pkgconf curl file patchelf \
      gtk4 libadwaita ffmpeg libplacebo libass vapoursynth zimg lcms2 uchardet \
      wayland wayland-protocols libxkbcommon libxpresent libxss libxv libxrandr \
      libdrm mesa vulkan-headers vulkan-icd-loader libva libvdpau libdisplay-info \
      pipewire libpulse alsa-lib systemd-libs desktop-file-utils
    ```
  - Other distros (Debian 13+, Ubuntu 24.10+, Fedora 41+): install the matching development packages for the libraries and tools listed above (Debian 12 lacks `libadwaita` ≥ 1.6 and VapourSynth headers).
- **Windows (MSYS2 UCRT64)** (uses MSYS2's `mpv`, which includes VapourSynth):
  ```sh
  pacboy -S rust:p pkgconf:p gcc:p gtk4:p libadwaita:p mpv:p \
    gdk-pixbuf2:p librsvg:p ntldd:p adwaita-icon-theme:p hicolor-icon-theme:p
  ```
  (Install `zip` as well if running `packaging/package-windows.sh`.)
- **macOS (Homebrew)** (uses Homebrew's `mpv`, which includes VapourSynth):
  ```sh
  brew install rust pkgconf gtk4 libadwaita adwaita-icon-theme hicolor-icon-theme mpv librsvg bash
  ```

## Installation

From [Releases](https://github.com/krakerz/EmbyClientPlus/releases):

| System | File | Install |
|---|---|---|
| Linux (rolling: Arch, CachyOS, …) | `EmbyClientPlus-<version>-x86_64.AppImage` | `chmod +x`, then run it |
| Linux (older or fixed-release: Debian, Ubuntu LTS, …) | `EmbyClientPlus-<version>-x86_64-compat.AppImage` | `chmod +x`, then run it |
| Linux (current glibc: Arch, Fedora, openSUSE Tumbleweed, …) | `embyclientplus-<version>-linux-x86_64.tar.gz` | extract, `./install.sh` (`./uninstall.sh` removes it); needs GTK 4.12+ and libadwaita 1.6+ |
| Windows | `embyclientplus-<version>-windows-x86_64.zip` | extract anywhere, run `Emby Client+.exe` |
| macOS (Apple Silicon) | `embyclientplus-<version>-macos-arm64.zip` | extract, move `Emby Client+.app` to Applications |

Which AppImage? Both carry their own GTK, libadwaita and FFmpeg. The plain one uses the system's
glibc, so it needs a current one; if it stops with `GLIBC_2.xx not found`, use the compat one,
which brings its own; it's tested on the latest Debian stable and Ubuntu LTS (Debian 13, Ubuntu
26.04). SVP inside the compat one
needs a system with glibc 2.38 or later (SVP's own requirement). The tar.gz uses the system's GTK
and libadwaita and brings the player's own libraries (FFmpeg, libplacebo, …); like the plain
AppImage it needs a current glibc and says so at start if the system's is too old.

macOS: the app isn't notarized; allow it once in System Settings → Privacy & Security → **Open
Anyway** (or `xattr -dr com.apple.quarantine "/Applications/Emby Client+.app"`).

Every build updates itself (Preferences → General → Updates). SVP 4 is optional and found in its usual
folder (`~/SVP4`, `C:\Program Files\SVP 4`, `/Applications/SVP 4 Mac.app`) or one you set. On
macOS, SVP also needs VapourSynth from Homebrew (`brew install vapoursynth`), as SVP for Mac does.

## Building from source

Install the [Build dependencies](#build) for your platform, then clone the repository:

```sh
git clone https://github.com/krakerz/EmbyClientPlus.git
cd EmbyClientPlus
```

**Linux:**

```sh
scripts/build-libmpv.sh && cargo build --release --locked && target/release/embyclientplus
EMBYCLIENTPLUS_PORTABLE=1 cargo build --release --locked && packaging/package.sh   # release packages → dist/
```

`scripts/build-libmpv.sh` builds mpv 0.42.0 into `third_party/`. Run it again after your distribution
updates FFmpeg or libplacebo (a "Could not find dependency: libplacebo.so.N" error when packaging
means the old build is linked against a library that's gone).

**Windows (MSYS2 UCRT64):**

```sh
cargo build --release --locked
packaging/package-windows.sh   # optional release zip → dist/
```

**macOS (Homebrew):**

```sh
LIBRARY_PATH="$(brew --prefix)/lib" cargo build --release --locked
packaging/package-macos.sh     # optional release zip → dist/
```

See `.github/workflows/build.yml` for the full CI build and packaging setup.

## Usage

Launch the app, sign in to your Emby server (remembered across runs), pick a title and press **Play** — or play a local file or stream directly without an Emby server:

```sh
embyclientplus /path/to/video.mkv
```

App settings live in Home's menu → **Preferences**; your Emby account's settings are on **Profile** (the avatar, top right).

### Command line

```sh
embyclientplus                         # start the app
embyclientplus --player [<file|url>]   # just the player (no Emby); plays the next video in the folder after each one
embyclientplus <file|url>              # same as --player <file|url>
embyclientplus -- <file>               # same, for a file named like an option
embyclientplus -h | --help
embyclientplus -v | --version
```

### Configuration

- **Downloads:** the download button on a movie or episode page, or **Download** in any item's
  menu (X on a controller, right-click). A series' Download menu also has **Auto-download…** and **Download Quality…**; its settings button sets how its **Intros** are handled.
  Files go to `Videos/Emby Client+` (Preferences → General → Downloads, which also sets the quality); Home's menu → **Downloads** lists
  them.
- **Video:** Preferences → Video picks a preset (**Auto**, **Fast** for Steam Deck, **Balanced**,
  **High quality**, **Custom**). The player's **Shaders** button picks one preset per group
  (FSR, Anime4K, Chroma upscaling, Luma upscaler, Sharpening, yours), remembered per title. Your
  own shaders go in the `shaders` folder next to `config.toml`: a folder per group, a `.glsl` file
  (or a subfolder of them) per preset. The bundled shaders keep their authors' licences (MIT,
  BSD-3-Clause, LGPL-3.0); each group's `LICENSE` is in `data/shaders/<group>/` and next to the
  installed copies.
- **Server identity:** the server lists the app as Emby Web by default; turn off Preferences →
  General → Server → "Appear as a web browser" to list it as Emby Client+ (sign in again).
- **Steam Game Mode:** add the AppImage (or `~/.local/bin/embyclientplus`) as a non-Steam game with
  the **Gamepad** layout. Artwork is in `EmbyClientPlus-<version>-steam-library.zip` on the
  release page (and the archive's `steam-library/`); set it from the shortcut's library page.
  In Game Mode the display rate follows SVP's output frame rate and drawing past 60 fps turns on by
  itself when SVP's target can exceed 60 fps; pick the target in Preferences → SVP (restart the app
  after changing it). While drawing past 60 fps, the interface's animations are off so they can't
  take the GPU from playback.

### Trakt: use your own Client ID

Trakt limits how many requests each API app may make, so this app ships no Client ID of its own.
Making one takes a minute and is free.

1. On trakt.tv open **Settings → Your API Apps → New Application**.
2. Pick any name, set the Redirect URI to `urn:ietf:wg:oauth:2.0:oob`, tick the **/scrobble**
   permission and leave the rest empty.
3. **Save**, then copy the **Client ID** (not the secret).
4. In the app open Preferences → Tracking → Trakt → Client ID, paste it, press Enter, then
   **Log In**.

Changing the Client ID logs you out of Trakt. To spare your quota, watched episodes are queued and
sent together (every 15 minutes by default, or only when you quit; **Send updates**), and the live
"watching now" status is off unless you turn on **Show 'watching now' on Trakt**.

Plays made before you connected Trakt are added once with **Sync whole library to Trakt** (Preferences
→ Tracking → Trakt); only what Trakt lacks is sent, and from then on new plays, also from other apps,
sync automatically. With **Also sync unwatched** on, items you mark unplayed in Emby are also removed
from your Trakt history (found by the weekly full check).

### Keyboard and controller

Both are remappable (Preferences → Keyboard, Controller); the legend at the bottom left shows the
current screen's buttons.

| Key | Action | Button | Browsing | Player |
|---|---|---|---|---|
| Space / K | Play/pause | A | Select | Play/pause |
| ← / → | Seek ±10 s | B | Back | Leave player |
| ↑ / ↓ | Volume ±5 | X | Item menu | Audio menu |
| M | Mute | Y | Home | Subtitle menu |
| , / . | Previous/next episode | LB / RB | Previous/next tab or season | Previous/next episode |
| PgUp / PgDn | Previous/next chapter | LT / RT | Notification (if one is showing), then music panel | Previous/next chapter |
| F / F11 | Fullscreen | Select | Search | Skip intro/credits |
| Q | Skip intro/credits | Start | Preferences | On-screen controls |
| Esc | Leave fullscreen, then the player | D-pad | Move | Seek (←/→), volume (↑/↓) |
| Ctrl+F (Cmd+F on macOS) or / | Search (outside the player) |  |  |  |
|  |  | Right stick | Scroll the row (←/→) or page (↑/↓) under the cursor | Scrub along the bar (seeks 2 s after you let go) |

## FAQ

**SVP doesn't kick in?** The SVP button shows why: green interpolating, amber waiting for SVP
Manager, red SVP Manager not running, grey SVP not found.

**Choppy playback or dropped frames?** The log notes the timing setup at each playback start; with
`EMBYCLIENTPLUS_STATS=1` it also carries a timing line every 10 seconds while playing.
For troubleshooting, these environment variables change the timing setup (read at startup):

- `EMBYCLIENTPLUS_VIDEO_SYNC=<mode>`: use this mpv `video-sync` mode instead of the app's choice (e.g. `audio`).
- `EMBYCLIENTPLUS_DISPLAY_FPS=<number>|off`: use this refresh rate for display timing, or `off` to set none.
  By default the app uses Preferences → General → Playback → Display refresh rate (in Steam Game Mode: SVP's output
  frame rate); this switch overrides both.
- `EMBYCLIENTPLUS_REPORT_SWAP=0`: stop reporting presented frames to mpv (also turns that check off).

**Where are settings and logs?** `config.toml` and `logs/` are in `~/.config/embyclientplus/`
(Linux), `%APPDATA%\EmbyClient+\` (Windows) or `~/Library/Application Support/EmbyClient+/` (macOS).

**Does auto-download convert files on the server?** No, it downloads the original files, like any
download, so the server does no transcoding.

**Why wasn't an episode synced?** It wasn't matched: the series page's tracking line says which
seasons are, and the AL / MAL / Kitsu / Trakt chips on each episode show its state per service (hover
for the entry). Unmatched episodes are skipped, never guessed.

**Can I use any shader in my own shader folder?** Not compute-shader files (e.g. ArtCNN): they
are skipped, with a warning in the log. Ordinary `.glsl` / `.hook` shaders work.

**Is HDR supported?** Not really: HDR plays converted to SDR, untested. SVP starts off for HDR
titles unless you turn it on for that title.

## License

Licensed under the GNU General Public License v3.0 or later ([LICENSE](LICENSE)).

Bundled shaders retain their respective licenses (MIT, BSD-3-Clause, LGPL-3.0; see `data/shaders/<group>/` and the installed copies). Bundled icons are from [Lucide](https://lucide.dev/) (ISC, `data/icons/LICENSE`).

---

### Notes

Built and maintained with the help of AI.
