{
  description = "YouTube-On-Kodi (yt-dlp, no API key) packaged for NixOS, with working playback";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.kodi.packages.callPackage ./package.nix { };
      });

      # Build the add-on for a specific Kodi variant (kodi-gbm, kodi-wayland, ...):
      #   youtube-on-kodi.lib.addonFor pkgs.kodi-gbm.packages
      lib.addonFor = kodiPackages: kodiPackages.callPackage ./package.nix { };

      nixosModules.default = import ./module.nix;
    };
}
