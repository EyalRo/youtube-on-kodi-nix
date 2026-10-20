# NixOS VM test for the UI spike: Slint straight on DRM/KMS (no compositor) on a 3840x2160
# virtio-gpu display at 2x scale, poster rows from a real mediad (Hebrew titles included),
# key-driven focus, and the handoff to `play` (real mpv, software output).
{ pkgs, appliance, ui, module }:

pkgs.testers.runNixOSTest {
  name = "media-appliance-ui";

  nodes.machine = { pkgs, ... }: {
    imports = [ module ];
    boot.initrd.kernelModules = [ "virtio_gpu" ];
    virtualisation.memorySize = 2048;
    virtualisation.qemu.options = [
      "-vga none"
      "-device virtio-gpu-pci,xres=3840,yres=2160"
    ];
    # No login prompt on the display the UI draws on (kiosk).
    systemd.services."getty@tty1".enable = false;
    systemd.services."autovt@".enable = false;
    fonts.packages = [ pkgs.noto-fonts pkgs.dejavu_fonts ];
    environment.systemPackages = [ appliance ui pkgs.mpv pkgs.ffmpeg pkgs.jq pkgs.sqlite ];
    systemd.tmpfiles.rules = [ "d /media/movies 0755 root root -" ];
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

    def ui_unit(handoff):
        machine.execute("systemctl stop ui.service; systemctl reset-failed ui.service")
        machine.execute("rm -rf /tmp/appliance-ui")
        machine.succeed(
            "systemd-run --unit=ui --collect"
            " -E SLINT_BACKEND=linuxkms-software -E SLINT_SCALE_FACTOR=2"
            f" -E UI_HANDOFF={handoff} -E UI_PLAY=play"
            " -E PLAYER_MPV_ARGS='--vo=drm --ao=null --hwdec=no' -E PLAYER_DRM_CONNECTOR=Virtual-1"
            " -E PATH=/run/current-system/sw/bin"
            " appliance-ui-session"
        )

    def ui_log():
        return machine.succeed("journalctl -u ui.service --no-pager -o cat")

    machine.wait_for_unit("mediad.service")
    machine.wait_for_file("/dev/dri/card0")
    movie("Alpha", 4, "0x3a6ea5")
    movie("Bravo", 4, "0xa53a6e")
    movie("סרט בדיקה", 4, "0x6ea53a")
    movie("Delta", 4, "0xa5a53a")
    machine.succeed(f"curl -sf -XPOST {API}/rescan")
    rows = api("/rows")["rows"]
    assert any(r["id"] == "movies" and r["count"] == 4 for r in rows), rows

    with subtest("the UI shows poster rows on the 4K display"):
        ui_unit("exit")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'rows loaded'", timeout=60)
        machine.sleep(8)
        machine.screenshot("ui-1-initial")
        assert "Rendering at 3840x2160" in ui_log(), ui_log()
        # A blank or text-console frame compresses to a few KB; a drawn UI does not.
        assert os.path.getsize(os.path.join(machine.out_dir, "ui-1-initial.png")) > 20000

    with subtest("arrow keys move the focus"):
        for k in ["right", "right", "down"]:
            machine.send_key(k)
            machine.sleep(1)
        machine.sleep(1)
        machine.screenshot("ui-2-moved")

    with subtest("choosing an item hands the display to play and the UI comes back"):
        machine.send_key("ret")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'handing the display to play'", timeout=30)
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -c 'rows loaded' | grep -q 2", timeout=90)
        machine.sleep(3)
        machine.screenshot("ui-3-back")
        assert machine.succeed("cat /tmp/appliance-ui/focus").strip() == "1 2"
        assert "mpv exited" in ui_log() and "exit status: 0" in ui_log()
        assert "1" in machine.succeed("sqlite3 /persist/mediad/mediad.sqlite 'select count(*) from play_state where watched=1'")

    with subtest("in-process handoff: does Slint release DRM master on hide()?"):
        ui_unit("inproc")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'rows loaded'", timeout=60)
        machine.sleep(2)
        machine.send_key("ret")
        machine.sleep(10)
        # Finding: hide() does not release DRM master, so mpv cannot open the display; this is
        # why the UI exits and a supervisor runs play. If Slint ever changes this, revisit.
        assert "playback failed" in ui_log(), ui_log()
  '';
}
