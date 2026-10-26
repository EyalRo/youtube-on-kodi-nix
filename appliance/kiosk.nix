# The living-room front end on real hardware: the UI (Slint on DRM/KMS) on tty1 under its own
# user and logind session, the way a Kodi box runs Kodi. Pairs with `nixosModules.appliance`
# (mediad, ytsync, artsync), which it expects on the same machine.
{ config, lib, pkgs, ... }:

let
  cfg = config.services.media-appliance.kiosk;
  backend = config.services.media-appliance;
  inherit (lib) mkEnableOption mkOption mkIf types;
in
{
  options.services.media-appliance.kiosk = {
    enable = mkEnableOption "the appliance UI on the main display";

    package = mkOption {
      type = types.package;
      description = "Package providing appliance-ui and appliance-ui-session.";
    };

    user = mkOption {
      type = types.str;
      default = "appliance";
      description = "User the UI and the player run as; it gets the display, sound and input devices.";
    };

    drmConnector = mkOption {
      type = types.str;
      default = "HDMI-A-1";
      description = "DRM connector of the TV, for mpv.";
    };

    audioDevice = mkOption {
      type = types.str;
      default = "alsa/hdmi:CARD=PCH,DEV=0";
      description = "mpv audio device (`mpv --audio-device=help`).";
    };

    scaleFactor = mkOption {
      type = types.str;
      default = "2";
      description = "Slint scale factor: 2 for a 4K display, 1 for 1080p.";
    };

    mpvExtraArgs = mkOption {
      type = types.str;
      default = "";
      description = "Extra mpv arguments, appended after the player's defaults.";
    };

    victorialogs = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "192.168.0.39:9428";
      description = "VictoriaLogs host:port. Power events are sent there, and the whole journal is uploaded.";
    };
  };

  config = mkIf cfg.enable {
    users.users.${cfg.user} = {
      isNormalUser = true;
      description = "Appliance UI";
      hashedPassword = "!";
      extraGroups = [ "audio" "input" "render" "video" ];
    };

    # tty1 belongs to the UI, not to a login prompt.
    systemd.services."getty@tty1".enable = false;

    fonts.packages = [ pkgs.noto-fonts pkgs.dejavu_fonts ];
    environment.systemPackages = [ cfg.package pkgs.mpv pkgs.alsa-utils ];

    # The PAM "login" session gives the user logind access to the DRM and input devices.
    systemd.services.appliance-ui = {
      description = "Appliance UI (DRM/KMS)";
      wantedBy = [ "multi-user.target" ];
      after = [ "systemd-user-sessions.service" "mediad.service" "sound.target" ];
      wants = [ "mediad.service" ];
      conflicts = [ "getty@tty1.service" ];
      path = [ backend.package cfg.package pkgs.mpv pkgs.alsa-utils ];
      environment = {
        SLINT_BACKEND = "linuxkms-software";
        SLINT_SCALE_FACTOR = cfg.scaleFactor;
        UI_HANDOFF = "exit";
        # Absolute: the PAM login session resets PATH, so the unit's `path` never reaches the script.
        UI_PLAY = "${backend.package}/bin/play";
        UI_MEDIAD = backend.bind;
        PLAYER_MEDIAD = backend.bind;
        PLAYER_DRM_CONNECTOR = cfg.drmConnector;
        PLAYER_AUDIO_DEVICE = cfg.audioDevice;
      } // lib.optionalAttrs (cfg.mpvExtraArgs != "") {
        PLAYER_MPV_ARGS = cfg.mpvExtraArgs;
      } // lib.optionalAttrs (cfg.victorialogs != null) {
        UI_VLOGS = cfg.victorialogs;
      };
      serviceConfig = {
        User = cfg.user;
        PAMName = "login";
        TTYPath = "/dev/tty1";
        TTYReset = true;
        TTYVHangup = true;
        TTYVTDisallocate = true;
        StandardInput = "tty";
        # StandardInput=tty would make output default to the tty too, where the UI's and the
        # player's logs are lost; the journal is where they are read (and shipped from).
        StandardOutput = "journal";
        StandardError = "journal";
        ExecStart = "${cfg.package}/bin/appliance-ui-session";
        Restart = "always";
        RestartSec = 2;
      };
    };

    services.journald.upload = mkIf (cfg.victorialogs != null) {
      enable = true;
      settings.Upload.URL = "http://${cfg.victorialogs}/insert/journald";
    };
  };
}
