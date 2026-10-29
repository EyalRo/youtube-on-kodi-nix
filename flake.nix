{
  description = "YouTube-On-Kodi (yt-dlp, no API key) packaged for NixOS, with working playback";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";

  # Only for the Python crash experiment (e2e-stress-py314/py315): a recent
  # unstable channel tarball, which has Python 3.15.0 (this pin predates it).
  # A tarball from releases.nixos.org, not GitHub. Nothing else uses it.
  inputs.nixpkgs-unstable = {
    url = "https://releases.nixos.org/nixpkgs/nixpkgs-26.11pre1093421.f20cd90e28fe/nixexprs.tar.xz";
    flake = false;
  };

  outputs = { self, nixpkgs, nixpkgs-unstable }:
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

      testAddon = pkgs: kodi: kodi.packages.callPackage ./package.nix {
        yt-dlp = fakeYtDlp pkgs;
        port = testPort;
        masterDir = testMaster;
        maxHeight = 1080;
      };

      skin = pkgs: pkgs.kodi.packages.callPackage ./skin.nix { };

      # Kodi 21 is built against nixpkgs' default Python (3.13). The embedded
      # interpreter crashed with many concurrent widget scripts, so the crash
      # experiment (docs/wiki/Home-rows) also runs against a 3.12 build.
      kodiPy312 = pkgs: pkgs.kodi.override { python3Packages = pkgs.python312Packages; };

      # Kodi with what the e2e tests need. `stress` swaps in the 15-widget skin.
      kodiEnv = kodi: stress: trivial: kodi.withPackages (k: [
        k.inputstream-adaptive
        k.libretro-nestopia
        k.controller-topology-project
        (k.callPackage ./skin.nix ({ stressWidgets = stress; }
          // (if trivial then { stressPlugin = "plugin.video.yok-trivial"; } else { })))
      ]);

      # Real Kodi (headless, software GL) plays the packaged add-on.
      e2eFor = pkgs: { kodi ? pkgs.kodi, stress ? false, trivial ? false, name ? "yok-e2e", defaultTests ? "" }:
        pkgs.writeShellApplication {
          inherit name;
          runtimeInputs = [ pkgs.python3 pkgs.ffmpeg pkgs.xorg.xwd ];
          text = ''
            export YOK_ADDON=${testAddon pkgs kodi}/share/kodi/addons/plugin.video.myyoutubers
            export YOK_KODI=${kodiEnv kodi stress trivial}/bin/kodi
            export YOK_XVFB=${pkgs.xorg-server}/bin/Xvfb
            export YOK_MESA=${pkgs.mesa}
            export YOK_MASTER=${testMaster}
            export YOK_PORT=${toString testPort}
            ${if stress then "export YOK_STRESS=1" else ""}
            ${if trivial then "export YOK_TRIVIAL_ADDON=${./tests/e2e/trivial-addon}" else ""}
            cd ${./tests/e2e}
            exec python3 -B -m unittest -v ${defaultTests} "$@"
          '';
        };

      e2e = pkgs: e2eFor pkgs { };

      # The Python crash experiment: 15 widgets on Kodi's default Python and on 3.12.
      e2eStress = pkgs: e2eFor pkgs {
        name = "yok-e2e-stress"; stress = true; defaultTests = "test_stress";
      };
      e2eStress312 = pkgs: e2eFor pkgs {
        name = "yok-e2e-stress-py312"; kodi = kodiPy312 pkgs; stress = true; defaultTests = "test_stress";
      };

      # The same 15 widgets, but each runs a trivial plugin (imports nothing, empty
      # list): separates the cost of starting a script from the work it does.
      e2eStressTrivial = pkgs: e2eFor pkgs {
        name = "yok-e2e-stress-trivial"; stress = true; trivial = true; defaultTests = "test_stress";
      };

      # Newer Pythons come from the unstable nixpkgs: its stock Kodi is built
      # against 3.14, and 3.15 is Kodi rebuilt with python315 (final 3.15.0).
      # 3.15.0's package set with build-time test suites switched off. Several
      # fail on the new interpreter (meson-python, hypothesis, ...) and would
      # block numpy and pillow, which Kodi's wrapper needs. Only tests are
      # skipped; what the packages do at runtime is unchanged.
      python315Pkgs = u: (u.python315.override {
        packageOverrides = self: super:
          let
            noChecks = name: pkg:
              let r = builtins.tryEval (u.lib.isDerivation pkg && pkg ? overridePythonAttrs);
              in if r.success && r.value
                 then pkg.overridePythonAttrs (_: { doCheck = false; doInstallCheck = false; })
                 else pkg;
          in builtins.mapAttrs noChecks super;
        self = u.python315;
      }).pkgs;

      unstable = pkgs: import nixpkgs-unstable { system = pkgs.stdenv.hostPlatform.system; };
      e2eStress314 = pkgs: let u = unstable pkgs; in e2eFor u {
        name = "yok-e2e-stress-py314"; stress = true; defaultTests = "test_stress";
      };
      e2eStressTrivial314 = pkgs: let u = unstable pkgs; in e2eFor u {
        name = "yok-e2e-stress-trivial-py314"; stress = true; trivial = true; defaultTests = "test_stress";
      };
      e2eStress315 = pkgs: let u = unstable pkgs; in e2eFor u {
        name = "yok-e2e-stress-py315"; stress = true; defaultTests = "test_stress";
        kodi = u.kodi.override { python3Packages = python315Pkgs u; };
      };
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.kodi.packages.callPackage ./package.nix { };
        e2e = e2e pkgs;
        e2e-stress = e2eStress pkgs;
        e2e-stress-py312 = e2eStress312 pkgs;
        e2e-stress-py314 = e2eStress314 pkgs;
        e2e-stress-py315 = e2eStress315 pkgs;
        e2e-stress-trivial = e2eStressTrivial pkgs;
        e2e-stress-trivial-py314 = e2eStressTrivial314 pkgs;
        skin = skin pkgs;
        # The appliance runs debug builds for now: the debug logs are compiled in and the default
        # log filter is debug. `appliance-release` and `appliance-ui-release` leave them out.
        appliance = pkgs.callPackage ./appliance/package.nix { debug = true; };
        appliance-ui = pkgs.callPackage ./appliance/ui/package.nix { debug = true; };
        appliance-release = pkgs.callPackage ./appliance/package.nix { };
        appliance-ui-release = pkgs.callPackage ./appliance/ui/package.nix { };
      });

      # nix run .#e2e
      apps = forAll (pkgs: {
        e2e = { type = "app"; program = "${e2e pkgs}/bin/yok-e2e"; };
        e2e-stress = { type = "app"; program = "${e2eStress pkgs}/bin/yok-e2e-stress"; };
        e2e-stress-py312 = { type = "app"; program = "${e2eStress312 pkgs}/bin/yok-e2e-stress-py312"; };
        e2e-stress-py314 = { type = "app"; program = "${e2eStress314 pkgs}/bin/yok-e2e-stress-py314"; };
        e2e-stress-py315 = { type = "app"; program = "${e2eStress315 pkgs}/bin/yok-e2e-stress-py315"; };
        e2e-stress-trivial = { type = "app"; program = "${e2eStressTrivial pkgs}/bin/yok-e2e-stress-trivial"; };
        e2e-stress-trivial-py314 = { type = "app"; program = "${e2eStressTrivial314 pkgs}/bin/yok-e2e-stress-trivial-py314"; };
      });

      # Build the add-on for a specific Kodi variant (kodi-gbm, kodi-wayland, ...):
      #   youtube-on-kodi.lib.addonFor pkgs.kodi-gbm.packages
      lib.addonFor = kodiPackages: kodiPackages.callPackage ./package.nix { };

      # The Estuary Rows skin, for kodi.withPackages:
      #   youtube-on-kodi.lib.skinFor pkgs.kodi-gbm.packages
      lib.skinFor = kodiPackages: kodiPackages.callPackage ./skin.nix { };

      # The appliance binaries CI compiled (the `latest` release), instead of building from source:
      #   (youtube-on-kodi.lib.prebuilt pkgs inputs.appliance-bin).appliance / .appliance-ui
      lib.prebuilt = pkgs: src: pkgs.callPackage ./appliance/prebuilt.nix { } src;

      nixosModules.default = import ./module.nix;

      # Media appliance back end (mediad + ytsync). Set `services.media-appliance.package`
      # to `youtube-on-kodi.packages.<system>.appliance`.
      nixosModules.appliance = import ./appliance/module.nix;

      # The UI on the main display (tty1, its own user and logind session), for real hardware;
      # set `services.media-appliance.kiosk.package` to `packages.<system>.appliance-ui`.
      nixosModules.appliance-kiosk = import ./appliance/kiosk.nix;

      # Hands-on QA VM (the NAS library from the workstation's /mnt/media): see appliance/qa-vm.nix
      nixosConfigurations.appliance-qa = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          ({ pkgs, ... }: import ./appliance/qa-vm.nix {
            inherit pkgs;
            appliance = pkgs.callPackage ./appliance/package.nix { };
            ui = pkgs.callPackage ./appliance/ui/package.nix { };
            module = ./appliance/module.nix;
            nfsShare = "192.168.0.100:/volume1/k8s/media-media-library-pvc-be51baa2-d7e2-4676-9de6-9961383f11bb";
          })
          "${nixpkgs}/nixos/modules/virtualisation/qemu-vm.nix"
        ];
      };

      # VM test: nix build .#checks.x86_64-linux.appliance -L
      checks.x86_64-linux.appliance-ui =
        let pkgs = nixpkgs.legacyPackages.x86_64-linux;
        in import ./appliance/test-ui.nix {
          inherit pkgs;
          appliance = pkgs.callPackage ./appliance/package.nix { };
          ui = pkgs.callPackage ./appliance/ui/package.nix { };
          module = ./appliance/module.nix;
        };

      # Spike: UI and player as Wayland clients under cage (no display handoff).
      checks.x86_64-linux.appliance-cage =
        let pkgs = nixpkgs.legacyPackages.x86_64-linux;
        in import ./appliance/test-cage.nix {
          inherit pkgs;
          appliance = pkgs.callPackage ./appliance/package.nix { };
          ui = pkgs.callPackage ./appliance/ui/package.nix { };
          module = ./appliance/module.nix;
          retroarch =
            let unfree = import nixpkgs { system = "x86_64-linux"; config.allowUnfree = true; };
            in unfree.retroarch.withCores (c: [ c.genesis-plus-gx ]);
          # Set CAGE_TEST_ROM=<path to a small Mega Drive ROM> and build with --impure to include the game subtest.
          rom = let p = builtins.getEnv "CAGE_TEST_ROM";
                in if p == "" then null else builtins.path { path = /. + p; name = "test-rom.smd"; };
        };

      # The Controllers page against a fake BlueZ.
      checks.x86_64-linux.appliance-bluetooth =
        let pkgs = nixpkgs.legacyPackages.x86_64-linux;
        in import ./appliance/test-bluetooth.nix {
          inherit pkgs;
          appliance = pkgs.callPackage ./appliance/package.nix { };
          ui = pkgs.callPackage ./appliance/ui/package.nix { };
          module = ./appliance/module.nix;
        };

      checks.x86_64-linux.appliance-handoff =
        let pkgs = nixpkgs.legacyPackages.x86_64-linux;
        in import ./appliance/test-handoff.nix {
          inherit pkgs;
          appliance = pkgs.callPackage ./appliance/package.nix { };
          module = ./appliance/module.nix;
        };

      checks.x86_64-linux.appliance-player =
        let pkgs = nixpkgs.legacyPackages.x86_64-linux;
        in import ./appliance/test-player.nix {
          inherit pkgs;
          appliance = pkgs.callPackage ./appliance/package.nix { };
          module = ./appliance/module.nix;
        };

      checks.x86_64-linux.appliance =
        let pkgs = nixpkgs.legacyPackages.x86_64-linux;
        in import ./appliance/test.nix {
          inherit pkgs;
          appliance = pkgs.callPackage ./appliance/package.nix { };
          module = ./appliance/module.nix;
        };
    };
}
