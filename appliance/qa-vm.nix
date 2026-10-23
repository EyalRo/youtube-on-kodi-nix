# Interactive VM for hands-on QA of the appliance: mediad plus the UI on the VM's display,
# with generated sample movies (Hebrew title included). Not a test; build and run it:
#   nix build .#nixosConfigurations.appliance-qa.config.system.build.vm
#   ./result/bin/run-appliance-qa-vm
# Click the QEMU window to give it the keyboard (arrows, Enter; ctrl+alt+g releases the grab).
# Debug shell: ssh -p 2222 root@localhost (empty password).
# With `nfsShare` (e.g. "192.168.0.100:/volume1/media") the library is that NFS export, mounted
# read-only at /media, instead of the generated samples.
{ pkgs, appliance, ui, module, nfsShare ? null }:

{
  imports = [ module ];

  networking.hostName = "appliance-qa";
  boot.initrd.kernelModules = [ "virtio_gpu" ];
  virtualisation.memorySize = 2048;
  virtualisation.cores = 2;
  virtualisation.qemu.options = [
    "-vga none"
    "-device virtio-gpu-pci,xres=1920,yres=1080"
    "-display gtk,zoom-to-fit=on"
    # Sound through the host's PipeWire, so the focus tick is audible.
    "-audiodev pipewire,id=snd0"
    "-device intel-hda"
    "-device hda-duplex,audiodev=snd0"
  ];
  virtualisation.forwardPorts = [{ from = "host"; host.port = 2222; guest.port = 22; }];

  services.openssh = { enable = true; settings.PermitRootLogin = "yes"; settings.PermitEmptyPasswords = "yes"; };
  security.pam.services.sshd.allowNullPassword = true;
  users.users.root.initialHashedPassword = "";

  # Kiosk: no login prompt on the display the UI draws on.
  systemd.services."getty@tty1".enable = false;
  systemd.services."autovt@".enable = false;
  fonts.packages = [ pkgs.noto-fonts pkgs.dejavu_fonts ];
  environment.systemPackages = [ appliance ui pkgs.mpv pkgs.jq pkgs.sqlite pkgs.alsa-utils ];

  services.media-appliance = { enable = true; package = appliance; rescanSeconds = 30;
    # Downloads need a writable library, so only with the generated one (the NAS mount is read-only).
    ytsync = pkgs.lib.mkIf (nfsShare == null) { enable = true; seedChannels = ./channels.default.conf; maxChannels = 1; };
  };

  # The NAS, as the real box will see it. Read-only so QA cannot touch the library; automount +
  # nofail so a missing NAS delays nothing at boot.
  fileSystems."/media" = pkgs.lib.mkIf (nfsShare != null) {
    device = nfsShare;
    fsType = "nfs";
    options = [ "ro" "vers=3" "nofail" "x-systemd.automount" "x-systemd.idle-timeout=600" "_netdev" ];
  };
  boot.supportedFilesystems = pkgs.lib.mkIf (nfsShare != null) [ "nfs" ];

  systemd.services.qa-media = pkgs.lib.mkIf (nfsShare == null) {
    description = "Generate QA sample library";
    wantedBy = [ "multi-user.target" ];
    before = [ "mediad.service" ];
    path = [ pkgs.ffmpeg pkgs.coreutils ];
    serviceConfig = { Type = "oneshot"; RemainAfterExit = true; };
    script = ''
      mk() { # title color
        d="/media/movies/$1 (2024)"
        [ -d "$d" ] && return
        mkdir -p "$d"
        ffmpeg -loglevel error -f lavfi -i testsrc=size=640x360:rate=24 \
          -f lavfi -i sine=frequency=440 -t 120 -c:v libx264 -preset ultrafast -c:a aac \
          "$d/$1 (2024).mkv"
        ffmpeg -loglevel error -f lavfi -i "color=c=$2:size=400x600" -frames:v 1 "$d/poster.jpg"
      }
      mk Alpha 0x3a6ea5; mk Bravo 0xa53a6e; mk "סרט בדיקה" 0x6ea53a
      mk Delta 0xa5a53a; mk Echo 0x3aa5a5; mk Foxtrot 0xa56e3a
      chmod -R a+rX /media
    '';
  };

  systemd.services.ui = {
    description = "Appliance UI session";
    wantedBy = [ "multi-user.target" ];
    after = [ "mediad.service" "systemd-udev-settle.service" ];
    wants = [ "mediad.service" ];
    path = [ appliance ui pkgs.mpv pkgs.alsa-utils ];
    environment = {
      SLINT_BACKEND = "linuxkms-software";
      UI_HANDOFF = "exit";
      UI_PLAY = "play";
      PLAYER_MPV_ARGS = "--vo=drm --hwdec=no";
      PLAYER_DRM_CONNECTOR = "Virtual-1";
    };
    serviceConfig = {
      # ALSA starts muted.
      ExecStartPre = "-${pkgs.runtimeShell} -c 'amixer -q sset Master 100% unmute; amixer -q sset PCM 100% unmute'";
      ExecStart = "${ui}/bin/appliance-ui-session";
      Restart = "always";
    };
  };
}
