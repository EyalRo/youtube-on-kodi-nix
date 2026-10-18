{
  description = "YouTube-On-Kodi (yt-dlp, no API key) packaged for NixOS, with working playback";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});

      # End-to-end test fixtures: the add-on is built against a fake yt-dlp and
      # fixed test values for the playlist server.
      testPort = 18099;
      testMaster = "/tmp/yok-e2e-master";

      # Answers the two yt-dlp queries the add-on makes with URLs of the
      # generated fixtures; $YOK_STATE/variant picks MPEG-TS or fMP4.
      fakeYtDlp = pkgs: pkgs.writeShellScriptBin "yt-dlp" ''
        variant=$(cat "$YOK_STATE/variant" 2>/dev/null || echo ts)
        case "$*" in
          *protocol*) printf '%s\n' "$YOK_MEDIA/$variant/video.m3u8" "$YOK_MEDIA/$variant/audio.m3u8" ;;
          *) printf '%s\n' "$YOK_MEDIA/$variant/video.m3u8" ;;
        esac
      '';

      testAddon = pkgs: pkgs.kodi.packages.callPackage ./package.nix {
        yt-dlp = fakeYtDlp pkgs;
        port = testPort;
        masterDir = testMaster;
        maxHeight = 1080;
      };

      skin = pkgs: pkgs.kodi.packages.callPackage ./skin.nix { };

      kodiWithIsa = pkgs: pkgs.kodi.withPackages (k: [ k.inputstream-adaptive k.libretro-nestopia k.controller-topology-project (k.callPackage ./skin.nix { }) ]);

      # Real Kodi (headless, software GL) plays the packaged add-on.
      e2e = pkgs: pkgs.writeShellApplication {
        name = "yok-e2e";
        runtimeInputs = [ pkgs.python3 pkgs.ffmpeg pkgs.xorg.xwd ];
        text = ''
          export YOK_ADDON=${testAddon pkgs}/share/kodi/addons/plugin.video.myyoutubers
          export YOK_KODI=${kodiWithIsa pkgs}/bin/kodi
          export YOK_XVFB=${pkgs.xorg-server}/bin/Xvfb
          export YOK_MESA=${pkgs.mesa}
          export YOK_MASTER=${testMaster}
          export YOK_PORT=${toString testPort}
          cd ${./tests/e2e}
          exec python3 -B -m unittest -v "$@"
        '';
      };
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.kodi.packages.callPackage ./package.nix { };
        e2e = e2e pkgs;
        skin = skin pkgs;
      });

      # nix run .#e2e
      apps = forAll (pkgs: {
        e2e = { type = "app"; program = "${e2e pkgs}/bin/yok-e2e"; };
      });

      # Build the add-on for a specific Kodi variant (kodi-gbm, kodi-wayland, ...):
      #   youtube-on-kodi.lib.addonFor pkgs.kodi-gbm.packages
      lib.addonFor = kodiPackages: kodiPackages.callPackage ./package.nix { };

      # The Estuary Rows skin, for kodi.withPackages:
      #   youtube-on-kodi.lib.skinFor pkgs.kodi-gbm.packages
      lib.skinFor = kodiPackages: kodiPackages.callPackage ./skin.nix { };

      nixosModules.default = import ./module.nix;
    };
}
