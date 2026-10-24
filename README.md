# Emby Client+

A native Emby client for Linux, Windows and macOS with SVP frame interpolation, built for the desktop and Steam Game Mode.

## Description

A GTK4/libadwaita Emby client with libmpv embedded in-process. Its libmpv has VapourSynth, so
SVP 4 attaches just as it does to a standalone mpv.

Icons: [Lucide](https://lucide.dev/) (ISC, `data/icons/LICENSE`), converted for GTK by
`scripts/update-icons.sh` (needs `picosvg`).

## Features

- **Browsing:** Home follows your Emby library order and visibility; library grids with remembered sort and filters (decade, HD/SD, subtitles, rating, recently added) and an A–Z strip, Suggestions and Collections tabs; series open on the season you're watching; details show original title, air dates, studios, genres, tags and each season's own overview when the server has them; cast, More Like This, Included In, favourites, History, search.
- **Playback:** direct play or transcode (Original down to 420 kbps), version picker, resume and watched state synced, chapters with seek-bar previews, Skip Intro/Credits, Up Next, sleep timer, aspect/zoom, black-bar blur/glow.
- **Picture:** quality presets or custom scalers/debanding, deinterlacing, FSR and Anime4K shaders plus your own, per title.
- **SVP:** attaches when SVP 4 is installed; per-title toggle with a status light; mpv frame blending as an alternative.
- **Profile:** your Emby account's own settings (languages, subtitle mode, autoplay, intro skipping, hidden watched items, library order), picture, password and PIN; shared with Emby's apps.
- **Tracking:** MyAnimeList, AniList, Kitsu and Trakt follow what you watch, including split anime seasons (matched through AniBridge) and downloads watched offline; MyAnimeList and AniList log in from Desktop Mode.
- **Downloads:** original files for offline play with the full player; pause and resume, resuming after the app closes, with disk space shown; auto-download keeps a series' next unwatched episodes and can delete watched ones; downloaded titles are badged.
- **Music:** mini player, queue, shuffle/repeat, gapless, playlists, Instant Mix, media keys and now-playing on every OS.
- **Controller & Steam Deck:** everything works with a gamepad, with on-screen hints and remappable buttons; Steam library artwork included.
- **Also:** standalone player for local files and links (`--player`), UI size 50–200%, light/dark theme, own `mpv.conf`, YouTube trailers (`yt-dlp`), self-updates.

## Installation

From [Releases](https://github.com/krakerz/EmbyClientPlus/releases):

| System | File | Install |
|---|---|---|
| Linux (rolling: Arch, CachyOS, …) | `EmbyClientPlus-<version>-x86_64.AppImage` | `chmod +x`, then run it |
| Linux (older or fixed-release: Debian, Ubuntu LTS, …) | `EmbyClientPlus-<version>-x86_64-compat.AppImage` | `chmod +x`, then run it |
| Linux | `embyclientplus-<version>-linux-x86_64.tar.gz` | extract, `./install.sh` (`./uninstall.sh` removes it); needs GTK 4.12+, libadwaita 1.6+, FFmpeg, libplacebo |
| Windows | `embyclientplus-<version>-windows-x86_64.zip` | extract anywhere, run `Emby Client+.exe` |
| macOS (Apple Silicon) | `embyclientplus-<version>-macos-arm64.zip` | extract, move `Emby Client+.app` to Applications |

Which AppImage? Both carry their own GTK, libadwaita and FFmpeg. The plain one uses the system's
glibc, so it needs a current one; if it stops with `GLIBC_2.xx not found`, use the compat one,
which brings its own; it's tested on the latest Debian stable and Ubuntu LTS (Debian 13, Ubuntu
26.04). SVP inside the compat one
needs a system with glibc 2.38 or later (SVP's own requirement). The tar.gz uses the system's
libraries.

macOS: the app isn't notarized; allow it once in System Settings → Privacy & Security → **Open
Anyway** (or `xattr -dr com.apple.quarantine "/Applications/Emby Client+.app"`).

Every build updates itself (Preferences → Updates). SVP 4 is optional and found in its usual
folder (`~/SVP4`, `C:\Program Files\SVP 4`, `/Applications/SVP 4 Mac.app`) or one you set. On
macOS, SVP also needs VapourSynth from Homebrew (`brew install vapoursynth`), as SVP for Mac does.

## Building from source

Linux needs Rust, `meson`, `ninja` and the dev files of GTK 4.12 or newer, libadwaita 1.6 or
newer, FFmpeg, libplacebo, libass and VapourSynth (Debian 13, Ubuntu 24.10, Fedora 41, Arch and
newer have them; Debian 12 doesn't):

```sh
scripts/build-libmpv.sh && cargo build --release && target/release/embyclientplus
EMBYCLIENTPLUS_PORTABLE=1 cargo build --release && packaging/package.sh   # release packages → dist/
```

Windows (MSYS2 UCRT64) and macOS (Homebrew) use their packaged `mpv`; see the `windows` and
`macos` jobs in `.github/workflows/build.yml` and `packaging/package-{windows,macos}.sh`.

## Usage

Sign in to your Emby server (remembered), pick a title, press **Play**. App settings are in Home's
menu → **Preferences**; your account's settings are on **Profile** (the avatar, top right).

- **Downloads:** the download button on a movie or episode page, or **Download** in any item's
  menu (X on a controller, right-click). A series' Download menu also has **Auto-download…**.
  Files go to `Videos/Emby Client+` (Preferences → Downloads); Home's menu → **Downloads** lists
  them.
- **Video:** Preferences → Video picks a preset (**Auto**, **Fast** for Steam Deck, **Balanced**,
  **High quality**, **Custom**). The player's **Shaders** button picks one preset per group (FSR,
  Anime4K, yours), remembered per title. Your own shaders go in the `shaders` folder next to
  `config.toml`: a folder per group, a `.glsl` file (or a subfolder of them) per preset.
- **Server identity:** the server lists the app as Emby Web by default; turn off Preferences →
  Server → "Appear as a web browser" to list it as Emby Client+ (sign in again).
- **Steam Game Mode:** add the AppImage (or `~/.local/bin/embyclientplus`) as a non-Steam game with
  the **Gamepad** layout. Artwork is in `EmbyClientPlus-<version>-steam-library.zip` on the
  release page (and the archive's `steam-library/`); set it from the shortcut's library page.

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
|  |  | Right stick | Scroll the row (←/→) or page (↑/↓) under the cursor | Scrub along the bar (seeks 2 s after you let go) |

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

**Where are settings and logs?** `config.toml` and `logs/` are in `~/.config/embyclientplus/`
(Linux), `%APPDATA%\EmbyClient+\` (Windows) or `~/Library/Application Support/EmbyClient+/` (macOS).

**Does auto-download convert files on the server?** No, it downloads the original files, like any
download, so the server does no transcoding.

**Why wasn't an episode synced?** It wasn't matched: the series page's tracking line says which
seasons are, and the AL / MAL / Kitsu / Trakt chips on each episode show its state per service (hover
for the entry). Unmatched episodes are skipped, never guessed.

**Is HDR supported?** Not really: HDR plays converted to SDR, untested. SVP starts off for HDR
titles unless you turn it on for that title.

---

### Notes

Built and maintained with the help of AI.
