# Spike VM test: the UI as a Wayland client under cage, with the player launched as a child
# (UI_HANDOFF=child) and also a Wayland client. Nothing releases the display, so the UI keeps
# its process, focus and page across a playback.
{ pkgs, appliance, ui, module, retroarch ? null, rom ? null }:

pkgs.testers.runNixOSTest {
  name = "media-appliance-cage";

  nodes.machine = { pkgs, ... }: {
    imports = [ module ];
    boot.initrd.kernelModules = [ "virtio_gpu" ];
    virtualisation.memorySize = 3072;
    virtualisation.cores = 10;
    virtualisation.qemu.options = [ "-vga none" "-device virtio-gpu-pci,xres=1920,yres=1080" ];
    hardware.graphics.enable = true;
    systemd.services."getty@tty1".enable = false;
    systemd.services."autovt@".enable = false;
    fonts.packages = [ pkgs.noto-fonts pkgs.dejavu_fonts ];
    environment.systemPackages = [ appliance ui pkgs.cage pkgs.mpv pkgs.coreutils pkgs.ffmpeg pkgs.jq pkgs.sqlite pkgs.procps pkgs.wlr-randr pkgs.alsa-utils ] ++ pkgs.lib.optional (retroarch != null) retroarch;
    boot.kernelModules = [ "snd-dummy" ];
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

    def shown(name, why):
        # The picture must reach the screen: a black frame compresses to a few hundred bytes.
        # Software GL needs a few seconds for its first frame, so look again for up to 30 s.
        for _ in range(15):
            machine.screenshot(name)
            if os.path.getsize(os.path.join(machine.out_dir, name + ".png")) > 5000:
                return
            machine.sleep(2)
        raise Exception(why)

    machine.wait_for_unit("mediad.service")
    machine.wait_for_file("/dev/dri/card0")
    # Long enough that the player is still in front when we look at it.
    movie("Alpha", 30, "0x3a6ea5")
    movie("Bravo", 30, "0xa53a6e")
    machine.succeed(f"curl -sf -XPOST {API}/rescan")

    with subtest("cage starts and the UI draws as a Wayland client"):
        # A page and focus saved before a reboot must not survive a fresh start in cage mode.
        machine.succeed("mkdir -p /tmp/appliance-ui && echo '2 1' > /tmp/appliance-ui/focus && echo 'c-stale 0 0' > /tmp/appliance-ui/detail")
        machine.succeed(
            "systemd-run --unit=ui --collect"
            " -E HOME=/root -E XDG_RUNTIME_DIR=/run/user-ui -E LIBSEAT_BACKEND=builtin"
            " -E LIBGL_ALWAYS_SOFTWARE=1 -E MESA_LOADER_DRIVER_OVERRIDE=llvmpipe -E WLR_RENDERER=pixman -E WLR_NO_HARDWARE_CURSORS=1"
            " -E SLINT_BACKEND=winit -E SLINT_SCALE_FACTOR=1"
            " -E UI_HANDOFF=child -E UI_PLAY=play -E PLAYER_WAYLAND=1"
            " -E PLAYER_MPV_ARGS='--vo=gpu --profile=fast --ao=null --hwdec=no' -E PLAYER_AUDIO_DEVICE=alsa/default"
            ${pkgs.lib.optionalString (retroarch != null) ''" -E PLAYER_RETROARCH=${retroarch}/bin/retroarch -E PLAYER_CORE_DIR=${retroarch}/lib/retroarch/cores"''}
            " -E PATH=/run/current-system/sw/bin"
            " sh -c 'mkdir -p /run/user-ui && chmod 700 /run/user-ui && exec cage -s -- appliance-ui'"
        )
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -q 'rows loaded'", timeout=90)
        machine.succeed("test ! -e /tmp/appliance-ui/detail && test ! -e /tmp/appliance-ui/focus")
        assert "page opened" not in ui_log(), ui_log()
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
        machine.sleep(3)
        shown("cage-2-playing", "playing frame is blank")
        # The 30 s clip must still be playing: the player did not die at start.
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

    ${pkgs.lib.optionalString (rom != null) ''
    with subtest("a game (Mega Drive ROM) runs RetroArch as a Wayland client, UI process unchanged"):
        machine.succeed("mkdir -p /media/roms/megadrive && cp ${rom} '/media/roms/megadrive/Test Game.smd'")
        refreshed = int(machine.succeed("journalctl -u ui.service --no-pager -o cat | grep -c 'rows refreshed' || true").strip())
        machine.succeed(f"curl -sf -XPOST {API}/rescan")
        rows = api("/rows")["rows"]
        print("rows:", [r["id"] for r in rows])
        gi = [i for i, r in enumerate(rows) if "game" in r["id"].lower()]
        assert gi, rows
        # The UI polls mediad; it must have loaded the game rows before the down keys can reach them.
        machine.wait_until_succeeds(f"test $(journalctl -u ui.service --no-pager -o cat | grep -c 'rows refreshed' || true) -gt {refreshed}", timeout=60)
        machine.sleep(2)
        # Focus came back on the movies row (row 1) after the last playback.
        for _ in range(gi[0] - 1):
            machine.send_key("down")
            machine.sleep(1)
        machine.send_key("ret")
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -c 'handing the display to play' | grep -q 2", timeout=30)
        machine.wait_until_succeeds("pgrep -f '[g]enesis_plus_gx_libretro'", timeout=60)
        # RetroArch under software GL takes a while to start; wait until the core is running frames.
        machine.wait_until_succeeds("grep -q SET_GEOMETRY /tmp/retroarch-*.log", timeout=120)
        machine.sleep(3)
        shown("cage-4-game", "game frame is blank")
        machine.succeed("pgrep -f '[g]enesis_plus_gx_libretro'")
        # RetroArch asks for a second press before it quits, and under software GL it samples keys only
        # once per slow frame, so hold each press for 300 ms and repeat the pair until it quits.
        for _ in range(4):
            machine.send_monitor_command("sendkey esc 300")
            machine.sleep(1)
            machine.send_monitor_command("sendkey esc 300")
            machine.sleep(8)
            if machine.execute("journalctl -u ui.service --no-pager -o cat | grep -c 'play returned' | grep -q 2")[0] == 0:
                break
        machine.wait_until_succeeds("journalctl -u ui.service --no-pager -o cat | grep -c 'play returned' | grep -q 2", timeout=30)
        machine.sleep(3)
        machine.screenshot("cage-5-back-from-game")
        assert machine.succeed("pgrep -f '^/run/current-system/sw/bin/appliance-ui$'").strip() == pid_before
        assert ui_log().count("rows loaded") == 1, ui_log()
    ''}
  '';
}
