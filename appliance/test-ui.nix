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
    environment.systemPackages = [ appliance ui pkgs.mpv pkgs.coreutils pkgs.ffmpeg pkgs.jq pkgs.sqlite ];
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

    with subtest("directions move the focus"):
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

    with subtest("a show opens a page with season tabs and episode thumbnails, and playing returns to it"):
        for s, e, name in [(1, 1, "Pilot"), (1, 2, "Second"), (2, 1, "Return")]:
            d = f"/media/tv/Gamma (2020)/Season {s}"
            machine.succeed(f"mkdir -p '{d}'")
            f = f"{d}/Gamma - S0{s}E0{e} - {name}"
            machine.succeed(f"ffmpeg -loglevel error -f lavfi -i testsrc=size=320x180:rate=10 -t 3 -c:v libx264 -preset ultrafast '{f}.mkv'")
        machine.succeed("ffmpeg -loglevel error -f lavfi -i color=c=0x884422:size=640x360 -frames:v 1 '/media/tv/Gamma (2020)/Season 1/Gamma - S01E01 - Pilot-thumb.jpg'")
        machine.succeed(f"curl -sf -XPOST {API}/rescan")
        eps = api("/rows/tvshows")["items"]
        assert len(eps) == 1, eps
        detail = api("/items/" + eps[0]["id"])
        assert sorted({e["season"] for e in detail["episodes"]}) == [1, 2], detail
        assert [e["has_art"] for e in detail["episodes"]] == [True, False, False], detail
        ui_unit("exit")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'rows loaded'", timeout=60)
        machine.sleep(3)
        machine.send_key("down")  # movies
        machine.sleep(1)
        machine.send_key("down")  # tv shows
        machine.sleep(1)
        machine.send_key("ret")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'page opened'", timeout=30)
        machine.sleep(3)
        machine.screenshot("ui-4-page")
        assert "\"seasons\":2" in ui_log(), ui_log()
        machine.send_key("up")      # the page opens on the episodes; up reaches the season tabs
        machine.sleep(1)
        machine.send_key("right")   # season 2 tab
        machine.sleep(2)
        machine.screenshot("ui-5-season2")
        machine.send_key("down")    # into the episodes
        machine.sleep(1)
        machine.send_key("ret")     # play S02E01
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'handing the display to play'", timeout=30)
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -c 'page opened' | grep -q 2", timeout=90)
        machine.sleep(3)
        machine.screenshot("ui-6-back-on-page")
        saved = machine.succeed("cat /tmp/appliance-ui/detail")
        assert saved.split()[1:] == ["1", "0"], saved + ui_log()
        # Back climbs one level at a time: episodes -> season tabs -> the show list.
        machine.send_key("esc")
        machine.sleep(1)
        assert machine.execute("test -e /tmp/appliance-ui/detail")[0] == 0, "first back left the page: " + ui_log()
        machine.send_key("esc")
        machine.sleep(1)
        assert machine.execute("test -e /tmp/appliance-ui/detail")[0] != 0, "second back stayed on the page: " + ui_log()
        # Back to movies only, so the next subtest's first tile is a movie again.
        machine.succeed("rm -rf /media/tv/*")
        machine.succeed(f"curl -sf -XPOST {API}/rescan")

    with subtest("the menu button toggles watched, and settings shows the version"):
        ui_unit("exit")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'rows loaded'", timeout=60)
        machine.sleep(3)
        before = int(machine.succeed("sqlite3 /persist/mediad/mediad.sqlite 'select count(*) from play_state where watched=1'").strip())
        machine.send_key("menu")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'menu opened'", timeout=30)
        machine.sleep(1)
        machine.screenshot("ui-7-menu")
        machine.send_key("ret")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'watched toggle applied'", timeout=30)
        after = int(machine.succeed("sqlite3 /persist/mediad/mediad.sqlite 'select count(*) from play_state where watched=1'").strip())
        assert after != before, (before, after, ui_log())
        machine.send_key("up")      # onto the top bar
        machine.sleep(1)
        machine.send_key("ret")     # settings
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'settings opened'", timeout=30)
        machine.sleep(1)
        machine.screenshot("ui-8-settings")
        assert "\"version\":\"0.5.2\"" in ui_log(), ui_log()

    with subtest("a restarted UI paints from the cached rows, and the splash covers the console"):
        ui_unit("exit")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'rows loaded'", timeout=60)
        assert '"from_cache":true' in ui_log(), ui_log()
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'rows refreshed from mediad'", timeout=30)
        # The session paints the splash into the framebuffer (what shows between processes) and
        # puts tty1 in graphics mode; the first pixel is the UI's background, #0e1318 as BGRX.
        machine.execute("systemctl stop ui.service")
        machine.succeed("fbsplash --vt-graphics")
        machine.succeed("fbsplash $(ls /nix/store/*appliance-ui*/share/appliance-ui/logo-1400x300.gray | head -1)")
        assert machine.succeed("head -c4 /dev/fb0 | od -An -tx1").split() == ["18", "13", "0e", "ff"]
        machine.screenshot("ui-9-splash")

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
