# NixOS module for the media appliance back end: `mediad` (library daemon) and
# `ytsync` (timer-driven YouTube download job).
{ config, lib, pkgs, ... }:

let
  cfg = config.services.media-appliance;
  inherit (lib) mkEnableOption mkOption mkIf types;
  ytsyncEnabled = cfg.ytsync.enable;
in
{
  options.services.media-appliance = {
    enable = mkEnableOption "the media appliance library daemon (mediad)";

    package = mkOption {
      type = types.package;
      description = "Package providing the mediad and ytsync binaries.";
    };

    user = mkOption {
      type = types.str;
      default = "mediad";
      description = "User the services run as. Needs read access to the library, and write access to the youtube folder if ytsync is enabled.";
    };

    group = mkOption {
      type = types.str;
      default = "mediad";
    };

    mediaRoot = mkOption {
      type = types.path;
      default = "/media";
      description = "Library root: movies/, tv/, youtube/, roms/. The services wait for this mount.";
    };

    stateDir = mkOption {
      type = types.path;
      default = "/persist/mediad";
      description = "Where the catalog database lives. Must be on a persisted path.";
    };

    bind = mkOption {
      type = types.str;
      default = "127.0.0.1:8730";
      description = "Address the HTTP API listens on.";
    };

    rescanSeconds = mkOption {
      type = types.ints.unsigned;
      default = 900;
      description = "How often to rescan the library; 0 only scans on request.";
    };

    releaseDbDir = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = "Directory of libretro .dat files, used for game release dates.";
    };

    logLevel = mkOption {
      type = types.str;
      default = "info";
      description = "Log filter (error, warn, info, debug, trace; per-target filters allowed). Logs are JSON on the journal.";
    };

    artsync = {
      enable = mkEnableOption "the poster artwork fetcher (local cache, never written to the library)";

      tmdbKeyFile = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "/persist/artsync/tmdb.key";
        description = "File holding a TMDB API key (v3) or read token (v4); a string, not a path, so the secret stays out of the Nix store. Without it only games get artwork.";
      };

      schedule = mkOption {
        type = types.str;
        default = "*-*-* 00/6:30:00";
        description = "systemd OnCalendar expression.";
      };

      maxLookups = mkOption {
        type = types.ints.unsigned;
        default = 0;
        description = "Lookups per run; 0 means all.";
      };
    };

    ytsync = {
      enable = mkEnableOption "the YouTube download job";

      channelsFile = mkOption {
        type = types.path;
        default = "/persist/ytsync/channels.conf";
        description = "Followed channels, one per line: @handle, channel URL or UC... id, optional `| name`.";
      };

      seedChannels = mkOption {
        type = types.nullOr types.path;
        default = null;
        description = "Channels file to copy to channelsFile when that file does not exist yet (never overwrites an edited list).";
      };

      stateDir = mkOption {
        type = types.path;
        default = "/persist/ytsync";
        description = "Download archive and work directory (local disk, persisted).";
      };

      schedule = mkOption {
        type = types.str;
        default = "*-*-* 04:00:00";
        description = "systemd OnCalendar expression.";
      };

      maxPerChannel = mkOption { type = types.ints.positive; default = 5; };
      maxAgeDays = mkOption { type = types.ints.positive; default = 30; };
      maxHeight = mkOption { type = types.ints.positive; default = 1080; };
      maxChannels = mkOption {
        type = types.ints.unsigned;
        default = 0;
        description = "Channels per run; 0 means all. Use 1 for a first live test.";
      };
    };
  };

  config = mkIf cfg.enable {
    users.users = mkIf (cfg.user == "mediad") {
      mediad = { isSystemUser = true; group = cfg.group; };
    };
    users.groups = mkIf (cfg.group == "mediad") { mediad = { }; };

    systemd.tmpfiles.rules =
      [ "d ${cfg.stateDir} 0750 ${cfg.user} ${cfg.group} -" ]
      ++ [ "d ${cfg.stateDir}/art 0750 ${cfg.user} ${cfg.group} -" ]
      ++ lib.optional ytsyncEnabled "d ${cfg.ytsync.stateDir} 0750 ${cfg.user} ${cfg.group} -"
      ++ lib.optional (ytsyncEnabled && cfg.ytsync.seedChannels != null)
        "C ${cfg.ytsync.channelsFile} 0640 ${cfg.user} ${cfg.group} - ${cfg.ytsync.seedChannels}";

    systemd.services.mediad = {
      description = "Media appliance library daemon";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];
      unitConfig.RequiresMountsFor = [ cfg.mediaRoot cfg.stateDir ];
      environment = {
        MEDIAD_MEDIA_ROOT = toString cfg.mediaRoot;
        MEDIAD_DB = "${cfg.stateDir}/mediad.sqlite";
        MEDIAD_BIND = cfg.bind;
        MEDIAD_RESCAN_SECS = toString cfg.rescanSeconds;
        MEDIAD_LOG = cfg.logLevel;
        MEDIAD_ART_CACHE = "${cfg.stateDir}/art";
      } // lib.optionalAttrs (cfg.releaseDbDir != null) {
        MEDIAD_RELEASE_DB = toString cfg.releaseDbDir;
      };
      serviceConfig = {
        ExecStart = "${cfg.package}/bin/mediad";
        User = cfg.user;
        Group = cfg.group;
        Restart = "on-failure";
        RestartSec = 5;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        NoNewPrivileges = true;
        ReadWritePaths = [ cfg.stateDir ];
        ReadOnlyPaths = [ cfg.mediaRoot ];
      };
    };

    systemd.services.ytsync = mkIf ytsyncEnabled {
      description = "Download new videos of followed YouTube channels";
      unitConfig.RequiresMountsFor = [ cfg.mediaRoot cfg.ytsync.stateDir ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      path = [ pkgs.yt-dlp pkgs.ffmpeg pkgs.deno ];
      environment = {
        YTSYNC_MEDIA_ROOT = toString cfg.mediaRoot;
        YTSYNC_STATE = toString cfg.ytsync.stateDir;
        YTSYNC_CHANNELS = toString cfg.ytsync.channelsFile;
        YTSYNC_YTDLP = "${pkgs.yt-dlp}/bin/yt-dlp";
        YTSYNC_MEDIAD = cfg.bind;
        YTSYNC_MAX_PER_CHANNEL = toString cfg.ytsync.maxPerChannel;
        YTSYNC_MAX_AGE_DAYS = toString cfg.ytsync.maxAgeDays;
        YTSYNC_MAX_HEIGHT = toString cfg.ytsync.maxHeight;
        YTSYNC_MAX_CHANNELS = toString cfg.ytsync.maxChannels;
        YTSYNC_LOG = cfg.logLevel;
        # yt-dlp keeps its cache here.
        XDG_CACHE_HOME = "${cfg.ytsync.stateDir}/cache";
      };
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${cfg.package}/bin/ytsync";
        User = cfg.user;
        Group = cfg.group;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        NoNewPrivileges = true;
        ReadWritePaths = [ cfg.ytsync.stateDir "${toString cfg.mediaRoot}/youtube" ];
        TimeoutStartSec = "6h";
      };
    };

    systemd.services.artsync = mkIf cfg.artsync.enable {
      description = "Fetch missing poster artwork into the local cache";
      after = [ "mediad.service" "network-online.target" ];
      wants = [ "network-online.target" ];
      requires = [ "mediad.service" ];
      path = [ pkgs.curl ];
      environment = {
        ARTSYNC_MEDIAD = cfg.bind;
        ARTSYNC_CACHE = "${cfg.stateDir}/art";
        ARTSYNC_MAX_LOOKUPS = toString cfg.artsync.maxLookups;
        ARTSYNC_LOG = cfg.logLevel;
        SSL_CERT_FILE = "/etc/ssl/certs/ca-bundle.crt";
      } // lib.optionalAttrs (cfg.artsync.tmdbKeyFile != null) {
        ARTSYNC_TMDB_KEY_FILE = cfg.artsync.tmdbKeyFile;
      };
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${cfg.package}/bin/artsync";
        User = cfg.user;
        Group = cfg.group;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        NoNewPrivileges = true;
        ReadWritePaths = [ "${cfg.stateDir}/art" ];
        TimeoutStartSec = "2h";
      };
    };

    systemd.timers.artsync = mkIf cfg.artsync.enable {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "2min";
        OnCalendar = cfg.artsync.schedule;
        Persistent = true;
      };
    };

    systemd.timers.ytsync = mkIf ytsyncEnabled {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnCalendar = cfg.ytsync.schedule;
        Persistent = true;
        RandomizedDelaySec = "15m";
      };
    };
  };
}
