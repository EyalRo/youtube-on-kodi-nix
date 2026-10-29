# Bluetooth Controller Setup for Shmetflix

**Status:** Planned  
**Tracking issue:** [#15 — Add controller-focused Bluetooth setup to Shmetflix configuration](https://github.com/EyalRo/youtube-on-kodi-nix/issues/15)

## Goal

Add Bluetooth setup to the existing Shmetflix configuration menu. The only intended use is pairing and reconnecting Bluetooth game controllers, not providing a general-purpose Bluetooth manager.

The appliance is a NixOS living-room media system on a NUC 14, running a Rust/Slint UI as a Wayland client under the cage kiosk compositor (no desktop session).

## Known controllers

- Black 8BitDo M30, six-button controller.
- Teal 8BitDo Lite 2.

Use illustrations of these two controllers in the interface where useful. Do not restrict discovery or pairing to these models; other compatible game controllers should work too.

## User experience

- Add a controller-focused Bluetooth screen inside the existing configuration menu.
- Provide a clear **Scan for controllers** action.
- Display discovered controller names and useful state, such as available, paired, or connected.
- Allow users to pair/connect a discovered controller and reconnect a previously paired controller.
- Filter discovery toward game controllers using Bluetooth device-class and service metadata where available. Handle incomplete or ambiguous metadata gracefully so a compatible controller is not silently excluded.
- Keep the entire flow usable in the kiosk. Do not require a desktop Bluetooth applet, a desktop session, a mouse, or a keyboard.

## System integration

- Use BlueZ and the appropriate NixOS Bluetooth service/configuration.
- Grant the UI only the system permissions needed to scan, pair, connect, and inspect controller state.
- Reuse the existing RetroArch/Libretro controller-input stack for games. Do not implement a new input layer or emulator.
- Continue using the existing mpv integration for video playback; Bluetooth setup should not change video playback.
- Verify controller recognition and input in the existing RetroArch setup.

## Acceptance criteria

- [ ] A user can scan for Bluetooth game controllers from the configuration menu.
- [ ] Discovered devices show their names and pairing/connection state.
- [ ] A user can pair and connect a controller, then reconnect a previously paired controller.
- [ ] The black 8BitDo M30 and teal 8BitDo Lite 2 are represented in the UI.
- [ ] Other compatible controllers remain supported; incomplete metadata alone does not exclude a device.
- [ ] The flow works in the cage kiosk without a desktop session, mouse, or keyboard.
- [ ] Paired controllers are usable in existing RetroArch games.
- [ ] Pairing and reconnection state persist across UI restarts and system reboots.

## Implementation context

- Main project: [youtube-on-kodi-nix](https://github.com/EyalRo/youtube-on-kodi-nix)
- UI: `appliance/ui/ui/app.slint` and `appliance/ui/src/main.rs`
- NixOS appliance module: `appliance/module.nix` and `appliance/kiosk.nix`
- Host configuration: the `stags/nixos` Forgejo flake, under `hosts/nuc14-appliance`
- The appliance runs RetroArch as a fullscreen Wayland client under cage, with the `udev` input driver (it reads `/dev/input` directly).

This document records the planned feature; it does not claim that Bluetooth setup has already been implemented.
