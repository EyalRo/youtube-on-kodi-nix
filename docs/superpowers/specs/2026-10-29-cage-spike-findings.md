# Cage spike findings: VM stage

Test: `nix build .#checks.x86_64-linux.appliance-cage -L` (appliance/test-cage.nix). Passes.

## Proven in the VM

- cage runs the UI (Slint `backend-winit-wayland`, software renderer) as a Wayland client: home screen draws, VM keyboard keys reach it.
- `UI_HANDOFF=child`: choosing a movie spawns `play`, which opens libmpv as a fullscreen Wayland client (`--vo=gpu --gpu-context=wayland`, llvmpipe). The clip's test pattern fills the screen (cage-2-playing.png); the UI process keeps its pid.
- Back (esc) ends playback; the same UI process is back on the same page and focus, rows loaded once (no restart, no cache repaint).
- No exit 10, no `launch` file, no `session.sh` loop, no splash glue was involved.

## Found along the way

- mpv `--vo=wlshm` is killed by cage ("wl_viewport.set_destination sent with invalid values"); use the GL output (llvmpipe in the VM).
- Mesa needs `LIBGL_ALWAYS_SOFTWARE=1` in the VM or the playing frame stays black (Zink fails to choose a device).
- The UI's `Command::status()` blocks its event loop while the child runs (fine for the spike; a real version should spawn and poll).
- The UI's `watch_remote` reads /dev/input itself, so Back/Home reach both the UI and the player in front. Needs a rule (UI ignores input while a child runs).
- Binaries are wrapped (`.appliance-ui-wrapped`, `.cage-wrapped`), so tests match by command line, not `pgrep -x`.

## Not proven (needs the NUC)

Transition timing and black frames at real scanout, RetroArch under cage (not yet tried in the VM either), game latency, VA-API, AC-3 passthrough, 4K and refresh matching.

## Update: RetroArch under cage (VM, Mega Drive ROM, genesis_plus_gx)

Test now also covers a game (CAGE_TEST_ROM=<rom> nix build --impure ...): choosing a game tile starts RetroArch as a fullscreen Wayland client (Frogger title screen in cage-4-game.png), the UI process keeps its pid, and Esc twice (RetroArch's quit confirmation) returns to the same UI on the Games row with the game tile focused.

Needed for it to work:
- PLAYER_WAYLAND=1 makes RetroArch use `video_driver = "gl"` and `input_driver = "udev"` (udev reads /dev/input, so quitting does not depend on Wayland keyboard focus, as on the DRM path today). With the Wayland keyboard driver the Esc keys never reached it.
- Mesa software GL (llvmpipe) at 1080p needs CPU: the VM has 10 cores; with 1 the mpv frame stayed black. RetroArch takes ~10 s to show its first frame there.

Found in the existing code (not cage-related): `run_game` appended `--verbose --log-file` after `--`, so RetroArch treated them as content paths and never wrote its log (the "follow the log live" feature saw nothing). Fixed: the flags now go before `--`. The log now shows real issues, e.g. audio driver init errors in the VM (no HDMI sound card; the VM uses snd-dummy) that deserve a look on the NUC.

Still for the NUC: transition timing at real scanout, game latency, VA-API, passthrough, real audio, 4K and refresh matching.
