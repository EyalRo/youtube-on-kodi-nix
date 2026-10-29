# Spike VM test: the UI as a Wayland client under cage, with the player launched as a child
# (UI_HANDOFF=child) and also a Wayland client. Nothing releases the display, so the UI keeps
# its process, focus and page across a playback.
{ pkgs, appliance, ui, module }:

pkgs.testers.runNixOSTest {
  name = "media-appliance-cage";

  nodes.machine = { pkgs, ... }: {
    imports = [ module ];
    boot.initrd.kernelModules = [ "virtio_gpu" ];
    virtualisation.memorySize = 3072;
    virtualisation.qemu.options = [ "-vga none" "-device virtio-gpu-pci,xres=1920,yres=1080" ];
    hardware.graphics.enable = true;
    systemd.services."getty@tty1".enable = false;
    systemd.services."autovt@".enable = false;
    fonts.packages = [ pkgs.noto-fonts pkgs.dejavu_fonts ];
    environment.systemPackages = [ appliance ui pkgs.cage pkgs.mpv pkgs.coreutils pkgs.ffmpeg pkgs.jq pkgs.sqlite pkgs.procps pkgs.wlr-randr ];
    systemd.tmpfiles.rules = [ "d /media/movies 0755 root root -" "d /media/tv 0755 root root -" ];
    services.media-appliance = { enable = true; package = appliance; rescanSeconds = 0; };
  };

  testScript = ''
    import json
    import os
    API = "http://127.0.0.1:8730"

    def api(path):
        return json.loads(machine.succeed(f"curl -sf {API}{path}"))

    def movie(title, seconds, color):
        d = f"/media/movies/{title} (2024)"
        machine.succeed(f"mkdir -p '{d}'")
        machine.succeed(f"ffmpeg -loglevel error -f lavfi -i testsrc=size=320x180:rate=10 -t {seconds} -c:v libx264 -preset ultrafast '{d}/{title} (2024).mkv'")
        machine.succeed(f"ffmpeg -loglevel error -f lavfi -i color=c={color}:size=400x600 -frames:v 1 '{d}/poster.jpg'")

    def ui_log():
        return machine.succeed("journalctl -u ui.service --no-pager -o cat")

    machine.wait_for_unit("mediad.service")
    machine.wait_for_file("/dev/dri/card0")
    # Long enough that the player is still in front when we look at it.
    movie("Alpha", 30, "0x3a6ea5")
    movie("Bravo", 30, "0xa53a6e")
    machine.succeed(f"curl -sf -XPOST {API}/rescan")

    with subtest("cage starts and the UI draws as a Wayland client"):
        machine.succeed(
            "systemd-run --unit=ui --collect"
            " -E XDG_RUNTIME_DIR=/run/user-ui -E LIBSEAT_BACKEND=builtin"
            " -E LIBGL_ALWAYS_SOFTWARE=1 -E MESA_LOADER_DRIVER_OVERRIDE=llvmpipe -E WLR_RENDERER=pixman -E WLR_NO_HARDWARE_CURSORS=1"
            " -E SLINT_BACKEND=winit -E SLINT_SCALE_FACTOR=1"
            " -E UI_HANDOFF=child -E UI_PLAY=play -E PLAYER_WAYLAND=1"
            " -E PLAYER_MPV_ARGS='--vo=gpu --ao=null --hwdec=no'"
            " -E PATH=/run/current-system/sw/bin"
            " sh -c 'mkdir -p /run/user-ui && chmod 700 /run/user-ui && exec cage -s -- appliance-ui'"
        )
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'rows loaded'", timeout=90)
        machine.sleep(5)
        machine.screenshot("cage-1-ui")
        assert os.path.getsize(os.path.join(machine.out_dir, "cage-1-ui.png")) > 20000

    with subtest("keys reach the UI through cage"):
        for k in ["right", "down"]:
            machine.send_key(k)
            machine.sleep(1)

    pid_before = machine.succeed("pgrep -f '^/run/current-system/sw/bin/appliance-ui$'").strip()

    with subtest("choosing a movie plays it in front while the UI process lives on"):
        machine.send_key("ret")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'handing the display to play'", timeout=30)
        machine.wait_until_succeeds("pgrep -x play", timeout=30)
        machine.sleep(5)
        machine.screenshot("cage-2-playing")
        # The picture must actually reach the screen: a black frame compresses to a few hundred bytes.
        assert os.path.getsize(os.path.join(machine.out_dir, "cage-2-playing.png")) > 5000, "playing frame is blank"
        # A 30 s clip must still be playing after 5 s: the player did not die at start.
        machine.succeed("pgrep -x play")
        assert machine.succeed("pgrep -f '^/run/current-system/sw/bin/appliance-ui$'").strip() == pid_before

    with subtest("Back ends the player and the same UI, page and focus are back"):
        machine.send_key("esc")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'play returned'", timeout=60)
        machine.sleep(3)
        machine.screenshot("cage-3-back")
        assert machine.succeed("pgrep -f '^/run/current-system/sw/bin/appliance-ui$'").strip() == pid_before
        assert "mpv exited" in ui_log(), ui_log()
        assert ui_log().count("rows loaded") == 1, ui_log()
  '';
}
