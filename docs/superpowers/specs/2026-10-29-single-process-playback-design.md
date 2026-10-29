# Single-process playback: remove the display handoff

Status: SUPERSEDED (2026-10-29). We chose a kiosk compositor (cage) with the UI, mpv and RetroArch as
Wayland clients instead of one process; see `2026-10-29-cage-spike-findings.md` and
`services.media-appliance.kiosk.display = "cage"`. Kept for the reasoning.

## Intent

Moving from the UI to a video or a game currently releases the display and another process captures it
(`appliance-ui` exits with status 10, `session.sh` runs `play`, `play` runs libmpv or RetroArch, the UI
restarts). Even with the splash painted between steps, the viewer sees a gap. **Smoothness of that
transition is the top priority**, above any feature RetroArch offers. The end state is one process that
owns the display from boot to shutdown.

## Goals

- `appliance-ui` is the only process that ever holds the DRM master. No exit, restart or splash between
  browsing and playing.
- Video plays in the same process, with the on-screen controls drawn by Slint over the picture.
- Games run in the same process through a built-in libretro host.
- Returning from playback lands on the same screen, focus and page the user left.
- Progress, resume position and "played" reporting to mediad behave as today.

## Non-goals

- Matching RetroArch's extras (save states, shaders, in-game menu, rewind, controller autoconfig). Add
  later only if wanted.
- Changing mediad, the catalog, the scanner or the sync jobs.
- External helpers that never touch the display (`systemctl`, `aplay`, `curl`, `yt-dlp`) may stay.

## Anti-goals

- Any design that still needs a second process to hold the display, even briefly, for video or games.
- Losing audio passthrough (AC-3/E-AC-3 to the TV) or VA-API decode.

## Design

1. **Renderer.** Switch the Slint dependency from `renderer-software` to `renderer-femtovg` on
   `backend-linuxkms` (GBM/EGL). Use `Window::set_rendering_notifier` to get the live GL context each frame.
2. **Video.** libmpv through its render API (`mpv_render_context`, OpenGL) on Slint's GL context. mpv draws
   into the current framebuffer before Slint paints; Slint draws the OSD and controls on top, replacing
   mpv's OSC. VA-API stays via mpv's GL interop. Audio options (`audio-spdif`, device) are unchanged.
3. **Games.** A libretro host in the player crate: `dlopen` the core named by `core_for`, run its frame
   loop, upload the video callback to a GL texture drawn in the same pass, send audio to ALSA on the TV
   device (48 kHz, 128 ms, as `retroarch_overrides` sets today), read input from evdev as `keys.rs` does.
   First version: play, quit, battery-save (SRAM) persistence. Start with fbneo, then the rest of `core_for`.
4. **Shared library.** `player` keeps one backend trait (start, per-frame, key, finish) with a video and a
   game implementation, and keeps the mediad reporting, resume logic and key mapping. The UI depends on it by
   path, as it does on `applog`. Licensing: the UI binary is already GPL-3.0-only; libmpv and the cores are
   loaded dynamically; check each core's licence before shipping it.
5. **Removal.** Delete the `play` binary, `UI_HANDOFF`, the `launch` file, exit code 10, the `session.sh`
   loop and the between-process splash, the loader-until-handover logic, and the `exit` mode tests.
   Keep the boot-time splash and tty graphics mode.

## Phases (each ends with something running on the NUC)

1. Spike (throwaway): Slint on femtovg under linuxkms, test pattern from the rendering notifier. Confirms
   EGL, the GL context and frame pacing on the target hardware.
2. Video in-process with a Slint playback screen.
3. Route video through it; keep the old `play` path behind a flag for games.
4. libretro host, fbneo first, then the other systems.
5. Delete `play`, the supervisor loop and the old tests; update `test-ui.nix`, `test-player.nix`,
   `test-handoff.nix`.

## Constraints and risks

- The GPU renderer costs more than software rendering and depends on EGL working on the NUC (mpv already
  uses it, so likely fine; phase 1 confirms).
- The libretro host is the largest piece and drops RetroArch features (see non-goals).
- Slint's rendering-notifier support on linuxkms with femtovg is assumed from its documentation and must be
  confirmed in the phase 1 spike. If it fails, fall back to drawing video on a DRM overlay plane owned by
  the same process, or to a Slint skia renderer.
- Controls and keys during playback currently go through mpv's IPC socket; they must move to direct calls.

## Left to the builder

Exact trait shape, GL texture handling, audio crate choice (alsa binding vs. cpal), the Slint playback
screen's look, and how VM tests fake the GL path.
