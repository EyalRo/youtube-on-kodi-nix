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
    libraryDir = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = ''
        Folder the add-on exports channels to as TV shows. Created for `user`
        if set. Must match the `libraryDir` the add-on package was built with
        (or the add-on setting). Add it once in Kodi as a Video source with
        content "TV shows" and the "Local information only" scraper.
      '';
    };
    gamesDir = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      description = ''
        ROM folder, one subfolder per system (NES, Genesis, ...). Created for
        `user` if set. Must match the `gamesDir` the add-on was built with.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.tmpfiles.rules =
      map (dir: "d ${dir} 0755 ${cfg.user} - -")
        (lib.filter (d: d != null) [ cfg.libraryDir cfg.gamesDir ]);

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
