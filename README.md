# EmbyClientPlus

A native Linux Emby client with SVP motion interpolation support.

## Description

EmbyClientPlus is a native GTK4/libadwaita Emby client for Linux that embeds libmpv for playback with built-in VapourSynth support, enabling SVP (SmoothVideo Project) motion interpolation during playback. It provides a lightweight alternative to web-based Emby clients with superior codec support and frame smoothing capabilities.

## Features

- Native Emby login and library browsing
- Direct stream preferred with server transcode fallback
- SVP motion interpolation support (requires SVP 4 installed at ~/SVP4 and running)
- Session-persistent playback tracking and resume positions

## Installation

Not yet available as a packaged release. Build from source (below) in the meantime.

## Building from source

Prerequisites: `meson`, `ninja`, a Rust toolchain, GTK4 and libadwaita development files, mpv's usual build dependencies, and SVP 4 at `~/SVP4`.

```sh
./scripts/build-libmpv.sh
cargo build --release
```

## Usage

### Launch the app

Run `embyclientplus` to start:
1. Log in with your server address, username, and password (token stored in system keyring)
2. Browse Home (libraries, continue watching, next up, latest items) — tap the menu for Preferences
3. Navigate series, search, and view item details
4. Click Play to start playback inside the same window

In Preferences you can set SVP and quality defaults, preferred audio and subtitle languages, clear per-title choices, and access logs.

### Player controls

The player shows an auto-hiding on-screen display (OSD) with a toolbar, hide after 3 s idle while playing; stays visible while paused or a menu is open. The OSD includes a seek bar with chapter marks, audio/subtitle track pickers that remember your choice per series/movie, quality presets, and controls for next/previous episodes.

**Keyboard:**
- **Space** or **K** — play/pause
- **←/→** — seek ±10 seconds
- **↑/↓** — volume
- **M** — mute
- **N/P** — next/previous episode
- **Page Down/Up** — next/previous chapter
- **F** or **F11** — toggle fullscreen
- **Esc** — exit fullscreen, then go back
- **Double-click** — toggle fullscreen

**Quality menu:** Original (direct stream, default), 20/10/6/3 Mbps. Capped presets request a server transcode and resume at the same position.

**Skip Intro / Skip Credits:** buttons appear when the playback position reaches intro or credits markers (from Emby's metadata). Skip Credits plays the next episode.

**Up Next card:** appears near the end of an episode with a 10 s countdown; auto-plays the next episode unless cancelled.

**SVP per-title toggle:** in the OSD controls. The button shows green while interpolating, amber while waiting for SVP Manager, red (with a one-time notice) when SVP Manager isn't running, or disabled if SVP isn't installed. Your choice is remembered per series/movie; the global default is set in Preferences.

### Standalone playback (file or URL)

```sh
embyclientplus <file-or-url>
```

Plays a local file or URL directly without Emby login. Useful for testing the player or SVP outside the client.

## FAQ

**Does this work on Wayland?**
Yes. The app uses native Wayland rendering with libmpv embedded in a GTK4 GLArea.

**How do I use SVP?**
Ensure SVP 4 is installed at `~/SVP4` and SVP Manager is running. The app automatically attaches via `/tmp/mpvsocket`.

---

### Notes

Built and maintained with the help of AI.
