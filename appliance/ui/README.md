# appliance-ui (spike)

Poster rows from `mediad`, drawn by [Slint](https://slint.dev) straight to DRM/KMS (no
compositor), driven by key events (libinput/evdev). Own Cargo workspace so Slint stays out of the
back end's build: `nix build .#checks.x86_64-linux.appliance-ui -L` runs it in a VM
(3840x2160 virtio-gpu, `SLINT_SCALE_FACTOR=2`, software renderer) and saves screenshots.

## Findings

- **Licence:** Slint's royalty-free licence excludes embedded systems, so this appliance can only
  use the GPL-3.0 option. The UI binary is therefore GPL-3.0-only as a whole; the
  GPL-2.0-or-later back end crates are compatible with that (the UI only talks to `mediad` over
  HTTP and runs `play`). A paid Slint licence would be the alternative.
- **Hebrew / RTL:** bidirectional text renders correctly with Noto/DejaVu fonts (fontconfig).
- **Display handoff:** `hide()` does not release DRM master, so `play` cannot start from inside
  the UI process. The UI exits (status 10, item id in `$UI_STATE/launch`, focus saved), the
  supervisor `appliance-ui-session` runs `play <id>`, then starts the UI again.
- **Console:** no getty may run on the display's VT; with one, the first frame never reached the
  screen.
- **Slint gotchas:** `row`/`col` are reserved property names; an element without an explicit `y`
  is centered in its parent.

## Not covered

Hardware rendering (femtovg/skia on the N150's iGPU), smoothness at 4K, the real remote's key
codes, libseat/unprivileged operation, async art loading (rows and art load synchronously at
start-up), watched marks and the on-screen keyboard.
