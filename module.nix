# NixOS module: serves the HLS master playlists the add-on writes, so Kodi
# opens them as an http stream (a local .m3u8 file is treated as a playlist).
{ config, lib, pkgs, ... }:

let
  cfg = config.services.youtube-on-kodi;
in
{
  options.services.youtube-on-kodi = {
    enable = lib.mkEnableOption "the playlist server for the YouTube-On-Kodi add-on";
    user = lib.mkOption {
      type = lib.types.str;
      default = "kodi";
      description = "User Kodi runs as; the add-on writes playlists as this user.";
    };
    port = lib.mkOption {
      type = lib.types.port;
      default = 8099;
      description = "Loopback port. Must match the `port` the add-on package was built with.";
    };
    runtimeDirectory = lib.mkOption {
      type = lib.types.str;
      default = "youtube-on-kodi";
      description = "Directory name under /run. Must match `masterDir` (/run/<name>) of the package.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.youtube-on-kodi-playlists = {
      description = "Playlist server for the YouTube-On-Kodi add-on";
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        User = cfg.user;
        RuntimeDirectory = cfg.runtimeDirectory;
        ExecStart = "${pkgs.python3}/bin/python3 -m http.server ${toString cfg.port} --bind 127.0.0.1 --directory /run/${cfg.runtimeDirectory}";
        Restart = "always";
      };
    };
  };
}
