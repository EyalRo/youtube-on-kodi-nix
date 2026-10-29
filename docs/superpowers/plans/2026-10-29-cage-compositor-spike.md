# Cage compositor spike

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Find out on the NUC whether running the UI, mpv and RetroArch as Wayland clients under one kiosk compositor (cage) gives a seamless transition and acceptable game latency, so we can drop the DRM handoff without writing our own game frontend.

**Architecture under test:** cage owns the DRM master for the whole session. `appliance-ui` runs as a Wayland client (Slint winit backend). The UI launches mpv or RetroArch as child processes; each is a fullscreen Wayland client. cage shows the focused fullscreen client, so a switch is one compositor frame and the UI stays mapped underneath.

**Tech Stack:** NixOS, cage (wlroots), Slint `backend-winit` + `renderer-femtovg` or software, mpv `--vo=gpu-next --gpu-context=wayland`, RetroArch `video_driver=gl` or `vulkan` with Wayland context.

**Spec:** `docs/superpowers/specs/2026-10-29-single-process-playback-design.md` (this spike decides whether it is superseded).

## Global Constraints

- Spike only: work on a branch `spike/cage`, nothing merged to `main` and nothing pushed or deployed without being asked.
- Reuse the existing UI, `play` binary, mediad and `kiosk.nix` options; do not rewrite them.
- Keep audio on the TV device (`audioDevice`), AC-3/E-AC-3 passthrough for mpv, VA-API decode.
- Measure, don't judge by eye alone: record numbers in the findings file.

## Review Focus

- Black or console frames visible during UI to player and back (the thing being fixed).
- Game input latency relative to today's DRM RetroArch.
- Remote keys (evdev) reaching the right client: UI vs the player in front.
- The UI stays responsive and keeps its focus/page when returning.
- Audio passthrough still works behind a compositor.

---

### Task 1: Bring up cage with the current UI

**Files:**
- Modify: `appliance/kiosk.nix` (spike branch: `programs.cage` or a `cage` service wrapping `appliance-ui`; env `SLINT_BACKEND=winit`; drop `SLINT_BACKEND=linuxkms-software`)
- Modify: `appliance/ui/Cargo.toml` (add `backend-winit`, keep `renderer-software` first; try femtovg only if software is slow)

- [ ] **Step 1:** Add `backend-winit` and a Wayland feature to the UI build on the spike branch; build.
- [ ] **Step 2:** Add a cage service running `appliance-ui` directly (no `session.sh` yet) as the `appliance-ui` user, with the same input/video/render groups.
- [ ] **Step 3:** Deploy to the NUC (test machine only) and confirm the home screen renders, remote navigation works, and the Plymouth to UI transition is clean.
- [ ] **Step 4:** Note frame rate and CPU in the findings file (Task 5).

### Task 2: Launch mpv and RetroArch as Wayland clients

**Files:**
- Modify: `appliance/crates/player/src/lib.rs` (spike-only env switch `PLAYER_WAYLAND=1`: mpv args use `--vo=gpu-next --gpu-context=wayland --fs`, RetroArch overrides use a Wayland-capable `video_driver`)
- Modify: `appliance/ui/src/main.rs` (spike-only: with `UI_HANDOFF=child`, spawn `play <id>` and keep running instead of exiting; re-enable input when the child exits)

- [ ] **Step 1:** Implement `UI_HANDOFF=child`: spawn `play`, show no loader, resume UI on exit.
- [ ] **Step 2:** Add the Wayland variants of the mpv and RetroArch arguments behind `PLAYER_WAYLAND`; the DRM path is unchanged when unset.
- [ ] **Step 3:** On the NUC: play a movie, quit, play a Neo Geo game, quit. Confirm the page and focus are kept.
- [ ] **Step 4:** Check that remote keys reach mpv/RetroArch while in front and the UI ignores them (fix with cage focus or by pausing UI input while a child runs).

### Task 3: Measure the transition

- [ ] **Step 1:** Record a high-speed phone video or capture-card clip of UI to mpv, mpv to UI, UI to RetroArch and back; count black/console frames.
- [ ] **Step 2:** Compare against the current exit-10 handoff, taken the same way.
- [ ] **Step 3:** Check `journalctl` for compositor or client errors during the switches.

### Task 4: Measure games and audio

- [ ] **Step 1:** Game input latency: use a button-to-flash test on the fbneo core (camera at 240 fps), or RetroArch's frame-delay/run-ahead stats, in DRM mode and under cage; record both.
- [ ] **Step 2:** Confirm the audio device, 48 kHz / 128 ms settings and no underruns in the RetroArch log under cage.
- [ ] **Step 3:** Play AC-3 content in mpv under cage and confirm receiver passthrough; confirm VA-API is in use (`--msg-level=vd=v` or mpv log).
- [ ] **Step 4:** Check 4K output and refresh-rate matching (`--display-fps-override`/cage mode) for a 24 fps film.

### Task 5: Decide

**Files:**
- Create: `docs/superpowers/specs/2026-10-29-cage-spike-findings.md`

- [ ] **Step 1:** Write the findings: transition (black frames, numbers), game latency delta, audio/passthrough/hwdec results, UI frame rate and CPU, any failures.
- [ ] **Step 2:** Recommend one of: adopt cage (then plan the production change: remove the supervisor, handoff and fbsplash glue); hybrid (libmpv in-process, RetroArch under cage); or return to the in-process plan. Commit the findings to the spike branch only.
- [ ] **Step 3:** Hand the findings to the user for the decision; do not start the production work.

---

## Self-review

- Covers the open questions from the discussion: transition smoothness, game latency, passthrough/hwdec, input routing, UI state kept.
- Spike changes are env-gated so the DRM path keeps working; nothing here removes existing code.
