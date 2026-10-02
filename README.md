# Emby Client+

A native Linux Emby client with SVP frame interpolation, built for the desktop and Steam Game Mode.

## Description

A GTK4/libadwaita Emby client with libmpv embedded in-process. Its libmpv is built with
VapourSynth, so SVP 4 attaches just as it does to a standalone mpv.

## Features

- Browsing:
  - Home rows;
  - tabbed libraries (genres, tags, collections, folders);
  - series and episodes, music, favourites, search.
- Music player: queue, shuffle/repeat, gapless, Emby playlists, media keys; no SVP.
- Plays the original file, with transcode presets; resume and watched state sync with Emby.
- Player:
  - chapters, track pickers remembered per title;
  - Skip Intro/Credits, Up Next, previous/next;
  - black-bar fill (blur/glow), aspect ratio and zoom per title.
- Theme: Light, Dark, or Follow system.
- SVP per title, with a status light. In Game Mode the app starts SVP Manager itself.
- Controller navigation, remappable; fullscreen under gamescope.
- Optional: your own `~/.config/mpv/mpv.conf`, and in-app trailers via `yt-dlp`.
- Self-updates from GitHub releases.

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

1. Start `embyclientplus` and sign in. The login is remembered.
2. Pick a title and press **Play** or **Resume**.

Settings are in Home's menu → **Preferences**: SVP, quality, languages, display, artwork,
mpv.conf, controller, updates and logs.

| Key | Action | Key | Action |
|---|---|---|---|
| Space / K | play/pause | N / P | next/previous episode |
| ← / → | seek 10 s | PgDn / PgUp | next/previous chapter |
| ↑ / ↓ / M | volume / mute | F / F11 | fullscreen |
| Esc | leave fullscreen, then back | double-click | fullscreen |

**Controller defaults:**

- **Browsing:** D-pad/stick move, A select, B back, Y home, X item menu, LB/RB tabs, Select search, Start Preferences.
- **Player:** A play/pause, D-pad seek/volume, LB/RB episode, LT/RT chapter, X/Y audio/subtitles, Select skip intro/credits, Start drives the on-screen buttons.

**Steam Game Mode:** add the AppImage (or the installed `bin/embyclientplus`) as a non-Steam
game, with the **Gamepad** layout.

`embyclientplus <file-or-url>` plays a file without Emby, for testing the player or SVP.

## FAQ

**SVP doesn't kick in?** The SVP button shows the state:

- green: interpolating;
- amber: waiting for SVP Manager;
- red: SVP Manager isn't running;
- grey: SVP not found.

**Where are settings and logs?** In `~/.config/embyclientplus/`: `config.toml` and `logs/`
(the newest 10 are kept).

**Does it bundle SVP?** No. It uses your own SVP install.

---

### Notes

Icons: [Lucide](https://lucide.dev/) (ISC, `data/icons/LICENSE`), converted for GTK by
`scripts/update-icons.sh` (needs `picosvg`).

Built and maintained with the help of AI.
