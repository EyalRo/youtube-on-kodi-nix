# Automated testing

Goal: as little human interaction as possible during development.

## Layers

1. **Unit smoke tests** (`python3 -m unittest discover -s tests`, ~0.1 s).
   Kodi's Python modules are stubbed; check stream resolution, playlist
   writing, listing context menus. Run anywhere.
2. **End-to-end** (`nix run .#e2e`, ~2 min). Real Kodi, no human, no GPU, no
   network:
   - Xvfb from nixpkgs with GLX, Mesa llvmpipe for software GL
     (`LIBGL_ALWAYS_SOFTWARE`, `LIBGL_DRIVERS_PATH`).
   - Kodi 21 (`kodi.withPackages [inputstream-adaptive]`) with the web server
     enabled; driven over JSON-RPC (`Player.Open`, `Player.Seek`,
     `Player.GetProperties`, `Addons.SetAddonEnabled`).
   - The add-on is the real Nix build (`package.nix`) with a fake `yt-dlp`
     that returns URLs of generated fixtures; the playlist server is a
     loopback `http.server`.
   - Fixtures: ffmpeg `testsrc2` + sine, split into video-only and audio-only
     VOD HLS, in MPEG-TS and fMP4 flavours.
   - Audio: an ALSA `null` PCM via `ALSA_CONFIG_PATH`. Without it Kodi loops
     on "no sink was returned" and the clock stalls.
3. **CI** (`.github/workflows/ci.yml`) runs build + both layers on every push.

## Gotchas learned

- Ubuntu's Xvfb has no GLX usable by Nix's Mesa; use nixpkgs `xorg-server`.
- The video player id is not constant; ask `Player.GetActivePlayers`.
- `Player.GetProperties` has `canseek`, not `seekable`.
- "Position reached the target" is not enough to prove a seek worked; also
  require `speed == 1` and that it keeps advancing.
- `pkill -f <pattern>` inside a tool command kills the shell running it.
- Kodi settings: an incomplete `guisettings.xml` is replaced by defaults; the
  web server still works.
- Enable debug logging (`advancedsettings.xml` `<loglevel>1`) or the HLS
  segment traces are missing.
- Keep the temp Kodi home with `YOK_KEEP=1` and read `.kodi/temp/kodi.log`.

## Test cases

play from start; seek forward while playing; seek back and near the end;
fMP4 seek via ISA (pass) and via ffmpeg (expected failure); resume from
history (position honoured, saved position not overwritten); "play from
start" ignoring history.

## Not covered yet / next

- Live-YouTube mode against real `yt-dlp` (needs network).
- Listing/navigation tests: `Files.GetDirectory` on `plugin://` paths to
  assert items, art and context menus.
- A `nixosTest` VM (module + systemd playlist server + `kodi-gbm`) needs KVM;
  not available in the sandbox. Worth adding to CI on a KVM runner.
- Screenshots: Kodi can capture via `Input.ExecuteAction`/`screenshot`
  builtin; useful for skin work later.

## Notes from the library, games, rows and skin tests

- `ExecuteAddon` is unreliable right after a fresh profile's first start and
  when several run at once (each pre-empts the previous). Prefer the
  background service (`media_refresh_minutes`) plus `VideoLibrary.Scan` for
  setup, and retry scans (Kodi drops a scan requested during another).
- A non-system skin is disabled until enabled; Kodi loads the skin before
  JSON-RPC is up and rewrites `guisettings.xml` on exit, so the harness
  enables it, quits, sets `lookandfeel.skin`, and relaunches.
- Kodi runs behind a wrapper script; the harness starts it in its own process
  group and kills the group, or an orphaned Kodi keeps the JSON-RPC port.
- `YOK_SCREENSHOTS=<dir>` saves screenshots (`xwd` + ffmpeg); `YOK_KEEP=1`
  keeps the temp Kodi home, `kodi.log` and any crash log.
