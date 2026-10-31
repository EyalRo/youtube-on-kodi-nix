# NixOS module for the media appliance back end: `mediad` (library daemon) and
# `ytsync` (timer-driven YouTube download job).
{ config, lib, pkgs, ... }:

let
  cfg = config.services.media-appliance;
  inherit (lib) mkEnableOption mkOption mkIf types;
  ytsyncEnabled = cfg.ytsync.enable;
  debugBuild = cfg.package.debug or false;
  # `VLOGS` is read by applog (the shared logging library) to push structured JSON to
  # VictoriaLogs; unset disables the push and leaves the journal as the only sink.
  vlogsEnv = lib.optionalAttrs (cfg.victorialogs != null) { VLOGS = cfg.victorialogs; };
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

    victorialogs = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "192.168.0.39:9428";
      description = "VictoriaLogs host:port. When set, mediad, ytsync and artsync push their logs there as structured JSON (service/host/level/fields queryable).";
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

    updateCheck = {
      enable = mkEnableOption "the upgrade check: fetches the newest published version number so the UI can badge the settings button";

      url = mkOption {
        type = types.str;
        default = "https://github.com/EyalRo/youtube-on-kodi-nix/releases/latest/download/VERSION";
        description = "A plain-text file holding the newest published appliance version (CI publishes it beside the binaries).";
      };

      file = mkOption {
        type = types.str;
        default = "/var/lib/appliance-update/available-version";
        description = "Where the fetched version is written (world-readable); the UI reads it.";
      };

      schedule = mkOption {
        type = types.str;
        default = if debugBuild then "*:0/5" else "*-*-* 04:30:00";
        defaultText = lib.literalExpression ''if the package is a debug build then "*:0/5" else "*-*-* 04:30:00"'';
        description = "systemd OnCalendar expression. Every 5 minutes on a debug build, daily otherwise.";
      };

      install = {
        flake = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "git+http://host:3000/me/nixos#nuc14";
          description = ''
            The flake of this machine's own configuration. When set, the settings page offers to install
            a newer version: `appliance-upgrade.service` runs `nixos-rebuild boot` on it (the new
            system boots next time; nothing restarts). Null disables installing from the UI.
          '';
        };

        overrideInputs = mkOption {
          type = types.attrsOf types.str;
          default = { };
          example = {
            appliance-bin = "https://github.com/EyalRo/youtube-on-kodi-nix/releases/download/latest/appliance-x86_64-linux.tar.gz";
            youtube-on-kodi = "github:EyalRo/youtube-on-kodi-nix";
          };
          description = ''
            Flake inputs of `install.flake` to re-fetch (`--override-input name url`) instead of using
            the pinned flake.lock, so the build gets the new release without committing a lock update.
          '';
        };
      };
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

      maxFilesize = mkOption {
        type = types.str;
        default = "3G";
        description = "yt-dlp --max-filesize: videos known to be bigger are skipped.";
      };
      playerClients = mkOption {
        type = types.str;
        default = "mweb,tv_simply,web_embedded";
        description = "yt-dlp YouTube player clients (comma separated). yt-dlp's own default currently gets HTTP 403 on video data; these download. Empty keeps yt-dlp's choice.";
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
      } // vlogsEnv
      // lib.optionalAttrs (cfg.releaseDbDir != null) {
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

    environment.systemPackages = lib.optional ytsyncEnabled (pkgs.writeShellScriptBin "ytsync-now" ''
      # Download one channel now (followed or not), as the service user with its hardening:
      #   ytsync-now @handle
      [ -n "$1" ] || { echo "usage: ytsync-now <@handle|url|UC-id>" >&2; exit 2; }
      systemctl set-environment YTSYNC_NOW="$1"
      systemctl start ytsync; rc=$?
      systemctl unset-environment YTSYNC_NOW
      exit $rc
    '');

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
        YTSYNC_PLAYER_CLIENTS = cfg.ytsync.playerClients;
        YTSYNC_MAX_FILESIZE = cfg.ytsync.maxFilesize;
        YTSYNC_MAX_AGE_DAYS = toString cfg.ytsync.maxAgeDays;
        YTSYNC_MAX_HEIGHT = toString cfg.ytsync.maxHeight;
        YTSYNC_MAX_CHANNELS = toString cfg.ytsync.maxChannels;
        YTSYNC_LOG = cfg.logLevel;
        # yt-dlp keeps its cache here.
        XDG_CACHE_HOME = "${cfg.ytsync.stateDir}/cache";
      } // vlogsEnv;
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
      } // vlogsEnv
      // lib.optionalAttrs (cfg.artsync.tmdbKeyFile != null) {
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

    # Only learns the version number; installing is a separate, verified step. A bad or missing reply
    # leaves the previous file alone, and anything that is not a dotted version is discarded.
    systemd.services.appliance-update-check = mkIf cfg.updateCheck.enable {
      description = "Look up the newest published appliance version";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      path = [ pkgs.curl pkgs.coreutils pkgs.gnugrep ];
      environment.SSL_CERT_FILE = "/etc/ssl/certs/ca-bundle.crt";
      script = ''
        out=${cfg.updateCheck.file}
        v=$(curl -fsSL --max-time 30 --max-filesize 64 ${lib.escapeShellArg cfg.updateCheck.url} | head -n1 | tr -d ' \r')
        printf '%s\n' "$v" | grep -Eq '^v?[0-9]+(\.[0-9]+){1,3}$' || { echo "unexpected reply: $v" >&2; exit 1; }
        printf '%s\n' "$v" > "$out.tmp"
        chmod 0644 "$out.tmp"
        mv "$out.tmp" "$out"
      '';
      serviceConfig = {
        Type = "oneshot";
        User = cfg.user;
        Group = cfg.group;
        StateDirectory = "appliance-update";
        StateDirectoryMode = "0755";
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        NoNewPrivileges = true;
        TimeoutStartSec = "2min";
      };
    };

    systemd.timers.appliance-update-check = mkIf cfg.updateCheck.enable {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "5min";
        OnCalendar = cfg.updateCheck.schedule;
        Persistent = true;
        RandomizedDelaySec = if debugBuild then "0" else "30m";
      };
    };

    # Started from the settings page (polkit rule in kiosk.nix). Builds the new system as the next
    # boot entry. `upgrade-status` is running/done/failed for the UI; `pending` names the version
    # waiting for a reboot (the UI shows it until it is the version that runs).
    systemd.services.appliance-upgrade = mkIf (cfg.updateCheck.enable && cfg.updateCheck.install.flake != null) {
      description = "Install the newest appliance version as the next boot entry";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      path = [ config.nix.package pkgs.nixos-rebuild pkgs.git pkgs.openssh pkgs.coreutils ];
      environment.HOME = "/root";
      script = ''
        dir=$(dirname ${cfg.updateCheck.file})
        mkdir -p "$dir"
        want=$(head -n1 ${cfg.updateCheck.file} | tr -d ' \r')
        echo running > "$dir/upgrade-status"
        if nixos-rebuild boot --refresh --flake ${lib.escapeShellArg cfg.updateCheck.install.flake} \
            ${lib.concatStringsSep " " (lib.mapAttrsToList (n: u: "--override-input ${lib.escapeShellArg n} ${lib.escapeShellArg u}") cfg.updateCheck.install.overrideInputs)}; then
          printf '%s\n' "$want" > "$dir/pending"
          echo done > "$dir/upgrade-status"
        else
          echo failed > "$dir/upgrade-status"
          exit 1
        fi
      '';
      serviceConfig = {
        Type = "oneshot";
        TimeoutStartSec = "1h";
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
