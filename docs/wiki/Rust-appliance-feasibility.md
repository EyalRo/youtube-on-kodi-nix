# Could a Rust appliance replace Kodi? (research, 2026-10-18)

Question: for a fixed-purpose experience (one resolution and refresh rate,
predefined media types and locations, YouTube channels, retro games, rows UI),
does Rust have what a Kodi replacement needs? **Verified** = read from
crates.io metadata or the projects' own repositories today. **Hypothesis** =
reasoned, not tested here.

## Existing Rust projects (verified by cloning each)

| Project | What it is | Verdict |
|---|---|---|
| KROMA | Rust server (axum, SQLite); the 10-foot UI is **React/TypeScript** shared across TVs; desktop/Steam Deck client is a **Tauri shell + native mpv** (VA-API hwdec, gamepad bridge) | Active (commit today). Closest match, but UI is web tech, not Rust. Same architecture as the one proposed below |
| Koko (LizardByte) | Rust server, Kodi/Plex-style browser UI planned | README: "NOT a functioning media server yet", "SHOULD NOT" be used; `client-desktop` is an empty `.keep` |
| Soia | Tauri + Vue desktop player on mpv (SMB/WebDAV/DLNA, web remote) | Active, a player, not a library/TV frontend |
| Dim | Rust media server, web UI | **Abandoned**: last commit 2020-04-04 |

No mature Rust application combines a TV-native UI, library, remote support,
games and add-ons. Everyone who ships uses Rust for the server and
web-tech or mpv for the screen.

## Crates for each capability (verified on crates.io)

| Need | Crate(s) | State |
|---|---|---|
| Video playback, HW decode, HDR, audio passthrough | `libmpv2` 6.0.0 (LGPL), mpv itself | updated 2026-05; the older `libmpv` is dead (2020) |
| Alternative engines | `gstreamer` 0.25.4 (12M downloads), `ffmpeg-next` 9.0 | both healthy |
| Direct display / mode setting | `drm` 0.15 (4M/90d), `gbm`, Slint `backend-linuxkms` | healthy |
| UI toolkit | **Slint 1.18** (made for embedded, has `backend-linuxkms`: draws straight to DRM/KMS, no compositor; libinput; Skia/femtovg/software renderers), `iced` 0.14, `egui` (immediate mode, poor fit), `bevy` 0.20 | Slint license: GPL-3.0-only OR royalty-free OR commercial |
| Audio out | `cpal`, `pipewire`, `alsa`, `symphonia` | healthy |
| Input | `evdev`, `gilrs`, `input` (libinput) | healthy |
| HDMI-CEC | `libcec` 8.1.7 (Pulse-Eight, GPL-2.0+, 2026-09); `cec-rs` is **deprecated** in its favour | official but young (113 downloads) |
| Library | `rusqlite`/`sqlx`, `walkdir`, `notify`, `quick-xml` (NFO), `tmdb-api`, `lofty`, `image`, `fast_image_resize`, `infer` | healthy |
| Services | `tokio`, `axum`, `reqwest`, `zbus` | healthy |
| YouTube | `yt-dlp` crate 2.9.0 (wraps the tool), `rustypipe` (Innertube, last 2025-04), `rusty_ytdl` (2024-08) | native clients lag yt-dlp; keep shelling out to yt-dlp |
| **Libretro (games) host** | `libretro-sys` (2018), `rust-libretro` (2023, for *writing cores*), `libretro-rs`, `libretro-backend` | **the gap**: nothing maintained for hosting cores |

## Assessment

- Rust has strong coverage for everything except a libretro **frontend**:
  the crates are for authoring cores or are unmaintained. Options: launch
  RetroArch (C) as an external process per game (how EmulationStation-style
  frontends work), or write a minimal host (hypothesis: a few thousand lines:
  dlopen via `libloading`, environment/video/audio/input callbacks).
- The hard part is **video plus UI on one screen**, not any single library
  (hypothesis, untested here). Simplest route: hand the display (DRM master) to
  mpv for playback and take it back after; mpv does refresh matching
  (`--drm-mode`, display-sync), HDR and S/PDIF passthrough itself.
- A fixed target removes most of what makes Kodi large: no skin engine, no
  add-on system, no PVR, no odd codecs, no multi-OS.
- Kodi stays C++ at its core; Python is only its add-on runtime, which is the
  crash surface we measured (docs/wiki/Home-rows).

## Recommended path

1. **Now, regardless of the end state:** move the add-on's logic into a
   standalone daemon (yt-dlp resolution, library export, games scan, catalog).
   It is the backend of either path, so the work is not wasted.
2. **Spike on the real target hardware before committing:** Slint
   `backend-linuxkms` poster grid + mpv DRM handoff + evdev/CEC input. Gate on:
   fixed mode held, 24p switching, HDR passthrough, UI/video handoff latency,
   grid scrolling smoothness on the target SoC.
3. Decide Kodi-with-daemon vs Rust appliance from the spike.

Not researched: Raspberry Pi/ARM specifics, DRM/Dolby Vision behaviour on a
particular TV, effort for a native on-screen keyboard and settings UI.
