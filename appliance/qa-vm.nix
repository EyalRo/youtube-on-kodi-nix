# Interactive VM for hands-on QA of the appliance, set up like the real box: the NAS library
# mounted at /media (movies, tv, roms read-only; youtube read-write for ytsync), mediad, the
# YouTube sync and the UI on the VM's display. Not a test; build and run it:
#   nix build .#nixosConfigurations.appliance-qa.config.system.build.vm
#   ./result/bin/run-appliance-qa-vm
# Click the QEMU window to give it the keyboard (arrows, Enter; ctrl+alt+g releases the grab).
# Debug shell: ssh -p 2222 root@localhost (empty password).
# Following creators: `ytsync follow @handle ["Name"]`, `ytsync unfollow @handle`, `ytsync list`
# (it edits the channels file); `systemctl start ytsync` downloads now instead of at 04:00.
{ pkgs, appliance, ui, module, nfsShare }:

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

  services.media-appliance = {
    enable = true;
    package = appliance;
    rescanSeconds = 30;
    # A first live test: a couple of videos from one channel, not the whole list.
    ytsync = { enable = true; seedChannels = ./channels.default.conf; maxChannels = 1; maxPerChannel = 2; };
  };

  # The export is mounted once, read-write, and /media is assembled from it: the library folders
  # are bind-mounted read-only so nothing but ytsync (youtube/) can change the NAS. automount +
  # nofail so a missing NAS delays nothing at boot.
  # (qemu-vm.nix replaces plain `fileSystems`; virtualisation.fileSystems is merged into it.)
  boot.supportedFilesystems = [ "nfs" ];
  virtualisation.fileSystems =
    let bind = sub: ro: {
      device = "/mnt/nas/${sub}";
      options = [ "bind" ] ++ pkgs.lib.optional ro "ro" ++ [ "nofail" ];
      depends = [ "/mnt/nas" ];
    };
    in {
      "/mnt/nas" = {
        device = nfsShare;
        fsType = "nfs";
        options = [ "vers=3" "nofail" "x-systemd.automount" "x-systemd.idle-timeout=600" "_netdev" ];
      };
      "/media/movies" = bind "movies" true;
      "/media/tv" = bind "tv" true;
      "/media/roms" = bind "roms" true;
      "/media/youtube" = bind "youtube" false;
    };
  # The youtube folder is the one place the box writes; world-writable because the NAS maps ids.
  systemd.tmpfiles.rules = [ "d /mnt/nas/youtube 0777 - - -" ];
  # Start mediad only once the library is mounted, not on an empty /media.
  systemd.services.mediad.unitConfig.RequiresMountsFor = [ "/media/movies" "/media/tv" "/media/roms" "/media/youtube" ];

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
      # Power events (armed, requested, accepted) go to VictoriaLogs as well as the console.
      UI_VLOGS = "192.168.0.39:9428";
    };
    serviceConfig = {
      # ALSA starts muted.
      ExecStartPre = "-${pkgs.runtimeShell} -c 'amixer -q sset Master 100% unmute; amixer -q sset PCM 100% unmute'";
      ExecStart = "${ui}/bin/appliance-ui-session";
      Restart = "always";
    };
  };
}
