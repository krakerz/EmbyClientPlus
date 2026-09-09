# EmbyClientPlus

A Linux-native Emby client with client-side frame interpolation and frame generation built into playback.

## Description

Emby's own web client handles library browsing well already, but no existing Emby client adds motion-smoothing or AI frame generation during playback. EmbyClientPlus embeds Emby's own web UI for browsing/login, then hands playback off to a native mpv-backed player that adds optional frame interpolation — either SVP-style motion interpolation (VapourSynth + mvtools) or lsfg-vk-style Vulkan frame generation — on top of a direct-play-first pipeline that lets mpv/ffmpeg handle far more codecs than a typical browser client.

**Status: early development.** No playback pipeline exists yet — see `CHANGELOG.md` for what's actually implemented so far.

## Features

- Emby library browsing and login via the server's own web client, embedded directly
- Native mpv-backed playback with optional frame generation, switchable between two backends:
  - SVP-style motion interpolation (VapourSynth + mvtools)
  - lsfg-vk-style Vulkan frame generation
- Per-title/series playback preference overrides (audio/subtitle language, frame-gen backend), on top of global defaults
- Native subtitle rendering independent of mpv, avoiding frame-gen ghosting on subtitle text

## Installation

Not yet available as a packaged release. Build from source (below) in the meantime.

## Building from source

Prerequisites: a Rust toolchain (stable, edition 2024 support) and the system packages Tauri's Linux backend needs (webkit2gtk, GTK3, and their `-dev` headers).

```sh
git clone git@github.com:krakerz/EmbyClientPlus.git
cd EmbyClientPlus
cargo build
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
