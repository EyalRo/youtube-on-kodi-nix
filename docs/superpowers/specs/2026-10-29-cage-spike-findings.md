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
