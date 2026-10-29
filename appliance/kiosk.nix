# The living-room front end on real hardware: the UI (Slint on DRM/KMS) on tty1 under its own
# user and logind session, the way a Kodi box runs Kodi. Pairs with `nixosModules.appliance`
# (mediad, ytsync, artsync), which it expects on the same machine.
{ config, lib, pkgs, ... }:

let
  cfg = config.services.media-appliance.kiosk;
  backend = config.services.media-appliance;
  inherit (lib) mkEnableOption mkOption mkIf types;

  # The Plymouth theme: the logo centred on the UI's background colour.
  shmetflixTheme = pkgs.runCommand "plymouth-theme-shmetflix" { } ''
    d=$out/share/plymouth/themes/shmetflix
    mkdir -p $d
    cp ${cfg.package}/share/appliance-ui/logo.png $d/logo.png
    cat > $d/shmetflix.plymouth <<EOF
    [Plymouth Theme]
    Name=Shmetflix
    Description=The Shmetflix logo on the UI background
    ModuleName=script

    [script]
    ImageDir=$d
    ScriptFile=$d/shmetflix.script
    EOF
    cat > $d/shmetflix.script <<EOF
    Window.SetBackgroundTopColor(0.055, 0.075, 0.094);
    Window.SetBackgroundBottomColor(0.055, 0.075, 0.094);
    logo = Image("logo.png");
    sprite = Sprite(logo);
    sprite.SetX(Window.GetWidth() / 2 - logo.GetWidth() / 2);
    sprite.SetY(Window.GetHeight() / 2 - logo.GetHeight() / 2);
    EOF
  '';

  # cage mode: set the wanted output mode (if any) before the UI draws its first frame.
  cageSession = pkgs.writeShellScript "appliance-cage-session" ''
    ${lib.optionalString (cfg.outputMode != null) ''
      ${pkgs.wlr-randr}/bin/wlr-randr --output ${cfg.drmConnector} --mode ${cfg.outputMode} || true
    ''}
    exec ${cfg.package}/bin/appliance-ui
  '';

  # Environment shared by both display modes: where mediad is, the player's devices and programs.
  uiEnv = {
    # Absolute: the PAM login session resets PATH, so the unit's `path` never reaches the script.
    UI_PLAY = "${backend.package}/bin/play";
    UI_MEDIAD = backend.bind;
    UI_UPDATE_FILE = backend.updateCheck.file;
    PLAYER_MEDIAD = backend.bind;
    PLAYER_AUDIO_DEVICE = cfg.audioDevice;
    PLAYER_RETROARCH = "${cfg.retroarch}/bin/retroarch";
    PLAYER_CORE_DIR = "${cfg.retroarch}/lib/retroarch/cores";
  } // lib.optionalAttrs (cfg.mpvExtraArgs != "") {
    PLAYER_MPV_ARGS = cfg.mpvExtraArgs;
  } // lib.optionalAttrs (cfg.victorialogs != null) {
    # Shared logging library reads VLOGS; covers the UI's own logs and, through the session,
    # the `play` player it launches.
    VLOGS = cfg.victorialogs;
  };
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

    bootSplash = mkOption {
      type = types.bool;
      default = true;
      description = "Quiet the console and show the Shmetflix logo (Plymouth) from the bootloader until the UI draws.";
    };

    display = mkOption {
      type = types.enum [ "drm" "cage" ];
      default = "drm";
      description = ''
        How the UI owns the display. "drm": the UI takes the DRM master on tty1, exits to hand the
        display to the player and restarts (a black gap on every playback). "cage": a kiosk
        compositor owns the display for the whole session and the UI, mpv and RetroArch are
        Wayland clients, so starting and leaving a video or game never releases the display.
      '';
    };

    outputMode = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "3840x2160@60";
      description = ''
        cage mode only: the output mode to set before the UI starts (`wlr-randr --mode`). Without it
        cage keeps the TV's preferred mode, which is often a 30 Hz one on 4K TVs. The output is `drmConnector`.
      '';
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

    # RetroArch with the libretro cores: NES, SNES, Genesis/SMS/Game Gear,
    # GB/GBC/GBA, and N64 (mupen64plus-next). snes9x and genesis-plus-gx are
    # `unfreeRedistributable`, so the host config must allow unfree
    # (nixpkgs.config.allowUnfree). Override with any
    # `pkgs.retroarch.withCores`-style package.
    retroarch = mkOption {
      type = types.package;
      default = pkgs.retroarch.withCores (cores: [
        cores.nestopia
        cores.snes9x
        cores.genesis-plus-gx
        cores.mgba
        cores.mupen64plus
        cores.fbneo
      ]);
      description = "RetroArch package (with libretro cores) used for game playback.";
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

    # Nothing but the logo on the screen from the bootloader to the first UI frame: no kernel or
    # systemd text on tty1, a Plymouth splash in the same colours as the UI (and as `fbsplash`, which
    # keeps the screen covered between the UI and the player).
    boot = mkIf cfg.bootSplash {
      kernelParams = [ "quiet" "loglevel=3" "udev.log_level=3" "systemd.show_status=false" "vt.global_cursor_default=0" ];
      consoleLogLevel = 0;
      initrd.verbose = false;
      plymouth = {
        enable = true;
        theme = "shmetflix";
        themePackages = [ shmetflixTheme ];
      };
    };

    # tty1 belongs to the UI, not to a login prompt.
    systemd.services."getty@tty1".enable = false;

    fonts.packages = [ pkgs.noto-fonts pkgs.dejavu_fonts ];
    environment.systemPackages = [ cfg.package cfg.retroarch pkgs.mpv pkgs.alsa-utils ];

    # drm mode: the PAM "login" session gives the user logind access to the DRM and input devices.
    systemd.services.appliance-ui = mkIf (cfg.display == "drm") {
      description = "Appliance UI (DRM/KMS)";
      wantedBy = [ "multi-user.target" ];
      after = [ "systemd-user-sessions.service" "mediad.service" "sound.target" ] ++ lib.optional cfg.bootSplash "plymouth-start.service";
      wants = [ "mediad.service" ];
      conflicts = [ "getty@tty1.service" ];
      path = [ backend.package cfg.package cfg.retroarch pkgs.mpv pkgs.alsa-utils ];
      environment = uiEnv // {
        SLINT_BACKEND = "linuxkms-software";
        SLINT_SCALE_FACTOR = cfg.scaleFactor;
        UI_HANDOFF = "exit";
        PLAYER_DRM_CONNECTOR = cfg.drmConnector;
      };
      serviceConfig = {
        # Plymouth lets go of the display (keeping its last frame up) just before the UI takes it.
        ExecStartPre = mkIf cfg.bootSplash "-+${pkgs.plymouth}/bin/plymouth quit --retain-splash";
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

    # cage mode: the compositor owns the display for the whole session; the UI is its only client
    # at start, and launches mpv / RetroArch as further fullscreen Wayland clients (UI_HANDOFF=child),
    # so nothing ever releases the display. nixpkgs' services.cage runs it on tty1 under the user's
    # logind session.
    services.cage = mkIf (cfg.display == "cage") {
      enable = true;
      user = cfg.user;
      program = "${cageSession}";
      environment = uiEnv // {
        SLINT_BACKEND = "winit";
        SLINT_SCALE_FACTOR = cfg.scaleFactor;
        UI_HANDOFF = "child";
        PLAYER_WAYLAND = "1";
      };
    };
    systemd.services."cage-tty1" = mkIf (cfg.display == "cage") {
      after = [ "mediad.service" "sound.target" ];
      wants = [ "mediad.service" ];
      path = [ backend.package cfg.package cfg.retroarch pkgs.mpv pkgs.alsa-utils ];
      serviceConfig = {
        # Plymouth lets go of the display just before the compositor takes it.
        ExecStartPre = mkIf cfg.bootSplash "-+${pkgs.plymouth}/bin/plymouth quit --retain-splash";
        StandardOutput = "journal";
        StandardError = "journal";
        Restart = "always";
        RestartSec = 2;
      };
    };

    # The top bar's sync button starts the download job (`systemctl start --no-block ytsync.service`)
    # as the UI user, which polkit refuses unless told otherwise. Only that one verb on that one unit.
    security.polkit = mkIf backend.ytsync.enable {
      enable = true;
      extraConfig = ''
        polkit.addRule(function(action, subject) {
          if (action.id == "org.freedesktop.systemd1.manage-units"
              && action.lookup("unit") == "ytsync.service"
              && action.lookup("verb") == "start"
              && subject.user == "${cfg.user}") {
            return polkit.Result.YES;
          }
        });
      '';
    };

    services.journald.upload = mkIf (cfg.victorialogs != null) {
      enable = true;
      settings.Upload.URL = "http://${cfg.victorialogs}/insert/journald";
    };
  };
}
