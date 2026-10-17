# youtube-on-kodi-nix

A Nix package and NixOS module for a maintained fork of
[XPDevs/Youtube-On-Kodi](https://github.com/XPDevs/Youtube-On-Kodi), a Kodi
add-on that browses and plays YouTube channels with yt-dlp: no Google API key
and no login.

The add-on source lives in [`addon/`](addon), forked from upstream commit
`954b94f` (v1.6.5). Compared with upstream:

1. **Plays on current YouTube and NixOS.** The add-on execs nixpkgs' `yt-dlp`
   wrapper directly, and since YouTube no longer offers any muxed audio+video
   format it requests an HLS video-only and audio-only pair and writes a small
   master playlist that Kodi plays as one stream (falling back to a muxed
   lookup if HLS isn't available).
2. **No "Playback failed" popup.** Playback goes through `setResolvedUrl`
   instead of `Player.play()`.
3. **Select plays.** Selecting a video plays it (history entries resume
   directly). Download, Add to playlist, Play from start and Remove from
   playlist are in the context menu, so every list works with Kodi's normal
   navigation and any skin.
4. **Seeking works.** Kodi's bundled ffmpeg (6.1) cannot seek in
   fragmented-MP4 HLS: after any seek the video breaks. The add-on plays
   through `inputstream.adaptive` instead (pulled in by the package; a setting
   switches back to ffmpeg), and resuming a video seeks to the saved position
   once playback starts (Kodi ignores `StartOffset` on plugin items).
5. **Quality setting.** Maximum quality (360p to 2160p) is an add-on setting;
   `maxHeight` in `package.nix` is only its default. Downloads honour it too.

Kodi treats a local `.m3u8` file as a playlist rather than a stream, so the
master playlists are served over loopback HTTP by a tiny systemd unit.

Ads: the add-on plays the streams yt-dlp resolves directly, so YouTube's ad
insertion isn't in the path. That is an observation, not a guarantee.

## Use

```nix
# flake.nix inputs
youtube-on-kodi.url = "github:EyalRo/youtube-on-kodi-nix";
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

`port`, `maxHeight` (360, 480, 720, 1080, 1440 or 2160) and `masterDir` are arguments of `package.nix` (defaults `8099` and
`/run/youtube-on-kodi`). If you change them, change the module's `port` and
`runtimeDirectory` to match.

In Kodi: Add-ons > Video add-ons > Youtube On Kodi, then add channels from the
add-on menu.

## Limitations

- Keep yt-dlp current; YouTube breaks it regularly.
- Playback picks the highest resolution up to the quality setting (default 2160), preferring H.264. Above 1080p YouTube only offers VP9/AV1 at high bitrates (about 28 Mbps for 4K60), so use a lower `maxHeight` if your hardware or network struggles. There is no adaptive switching.
- There are no real subscriptions, only a local channel list.
- Tested on Kodi 21 (Omega) with `kodi-gbm` on NixOS 25.11, x86_64.

## Development

- `python3 -m unittest discover -s tests`: fast smoke tests with Kodi's
  Python modules stubbed.
- `nix run .#e2e`: end-to-end tests. Starts a real Kodi under Xvfb with
  software GL, installs the Nix-built add-on with a fake `yt-dlp`, serves
  generated HLS fixtures (MPEG-TS and fMP4) and drives playback, seeking and
  resume over JSON-RPC. No network, GPU or human needed (about 2 minutes).
  `YOK_KEEP=1` keeps the temporary Kodi home and `kodi.log` for debugging.

CI runs both. Research notes and design docs are in [`docs/wiki/`](docs/wiki/Home.md). A skin
integrated with the add-on is planned but not started.

## License

GPL-2.0-or-later, matching upstream. The add-on in `addon/` is derived from upstream.
