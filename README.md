# EmbyClientPlus

A native Linux Emby client with SVP motion interpolation support.

## Description

EmbyClientPlus is a native GTK4/libadwaita Emby client for Linux that embeds libmpv for playback with built-in VapourSynth support, enabling SVP (SmoothVideo Project) motion interpolation during playback. It provides a lightweight alternative to web-based Emby clients with superior codec support and frame smoothing capabilities.

**Status: early development.** Core playback is functional (`embyclientplus <file-or-url>` to play). Emby login and library browsing are coming next.

## Features

- Play local files or URLs in an embedded player
- SVP motion interpolation support (requires SVP 4 installed at ~/SVP4 and running)

## Installation

Not yet available as a packaged release. Build from source (below) in the meantime.

## Building from source

Prerequisites: `meson`, `ninja`, a Rust toolchain, GTK4 and libadwaita development files, mpv's usual build dependencies, and SVP 4 at `~/SVP4`.

```sh
./scripts/build-libmpv.sh
cargo build --release
```

## Usage

Not yet functional end-to-end — usage instructions will follow once the playback pipeline lands.

## FAQ

**Why embed Emby's own web client instead of a native browsing UI?**
It already does browsing/search/theming well and stays in sync with whatever server version you run — reimplementing it natively would just be maintenance burden for no benefit.

**Does this work on Wayland?**
The primary window-embedding approach targets XWayland; a native-Wayland fallback is not yet implemented.

---

### Notes

Built and maintained with the help of AI.
