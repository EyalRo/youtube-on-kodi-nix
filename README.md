# youtube-on-kodi-nix

A Nix package and NixOS module for [XPDevs/Youtube-On-Kodi](https://github.com/XPDevs/Youtube-On-Kodi),
a Kodi add-on that browses and plays YouTube channels with yt-dlp: no Google
API key and no login.

The upstream add-on doesn't work as shipped on current YouTube and NixOS. This
repo packages it with three fixes:

1. **yt-dlp path.** The add-on runs `python <yt-dlp>`, but nixpkgs' `yt-dlp` is
   a bash wrapper. The package execs the wrapper directly.
2. **"Could not resolve stream".** YouTube no longer offers any muxed
   audio+video format, and the add-on only asks for one. The patch requests an
   HLS video-only and audio-only pair and writes a small master playlist that
   Kodi plays as one stream. It falls back to the original lookup if HLS isn't
   available.
3. **"Playback failed" popup over a working video.** The add-on called
   `Player.play()` without resolving the plugin item, so Kodi reported failure
   and stopped playback. The patch uses `setResolvedUrl` instead.

Kodi treats a local `.m3u8` file as a playlist rather than a stream, so the
master playlists are served over loopback HTTP by a tiny systemd unit.

Ads: the add-on plays the streams yt-dlp resolves directly, so YouTube's ad
insertion isn't in the path. That is an observation, not a guarantee.

## Use

```nix
# flake.nix inputs
youtube-on-kodi.url = "github:OWNER/youtube-on-kodi-nix";
```

```nix
# NixOS configuration
{ pkgs, lib, inputs, ... }:
let
  kodi = pkgs.kodi-gbm.withPackages (kpkgs: [
    (kpkgs.callPackage "${inputs.youtube-on-kodi}/package.nix" { })
    # or: inputs.youtube-on-kodi.lib.addonFor kpkgs
  ]);
in {
  imports = [ inputs.youtube-on-kodi.nixosModules.default ];
  services.youtube-on-kodi.enable = true;   # playlist server on 127.0.0.1:8099
  # services.youtube-on-kodi.user = "kodi"; # the user Kodi runs as

  environment.systemPackages = [ kodi ];
}
```

The add-on looks for `ffmpeg` and a JavaScript runtime on Kodi's `PATH`. If you
run Kodi as a systemd service, add them:

```nix
systemd.services.kodi.serviceConfig.Environment =
  [ "PATH=${lib.makeBinPath [ pkgs.ffmpeg pkgs.deno pkgs.coreutils ]}" ];
```

`port` and `masterDir` are arguments of `package.nix` (defaults `8099` and
`/run/youtube-on-kodi`). If you change them, change the module's `port` and
`runtimeDirectory` to match.

In Kodi: Add-ons > Video add-ons > Youtube On Kodi, then add channels from the
add-on menu.

## Limitations

- Upstream is a young single-maintainer project, pinned here to commit
  `954b94f` (v1.6.5). Keep yt-dlp current; YouTube breaks it regularly.
- Playback is capped at 720p.
- There are no real subscriptions, only a local channel list.
- Choosing Cancel in the Play/Download menu may show a harmless "playback
  failed" popup.
- Tested on Kodi 21 (Omega) with `kodi-gbm` on NixOS 25.11, x86_64.

## License

GPL-2.0-or-later, matching upstream. The patches here are derivative of the
add-on's code.
