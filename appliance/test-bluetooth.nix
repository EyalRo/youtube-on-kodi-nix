# VM test for the Controllers page: the UI under cage talks to a fake org.bluez on the system bus
# (test-fake-bluez.py). Drives Settings -> Controllers with the keyboard: scan, pair, forget, back.
{ pkgs, appliance, ui, module }:

let
  python = pkgs.python3.withPackages (p: [ p.dbus-fast p.evdev ]);
in
pkgs.testers.runNixOSTest {
  name = "media-appliance-bluetooth";

  nodes.machine = { pkgs, ... }: {
    imports = [ module ];
    boot.initrd.kernelModules = [ "virtio_gpu" ];
    boot.kernelModules = [ "uinput" ];
    virtualisation.memorySize = 3072;
    virtualisation.cores = 4;
    virtualisation.qemu.options = [ "-vga none" "-device virtio-gpu-pci,xres=1920,yres=1080" ];
    hardware.graphics.enable = true;
    systemd.services."getty@tty1".enable = false;
    systemd.services."autovt@".enable = false;
    fonts.packages = [ pkgs.noto-fonts pkgs.dejavu_fonts ];
    environment.systemPackages = [ appliance ui pkgs.cage pkgs.coreutils pkgs.ffmpeg pkgs.procps python ];
    systemd.tmpfiles.rules = [ "d /media/movies 0755 root root -" "d /media/tv 0755 root root -" ];
    services.media-appliance = { enable = true; package = appliance; rescanSeconds = 0; };
    # The real bluez package ships this policy; the fake needs the same permission.
    services.dbus.packages = [
      (pkgs.writeTextDir "share/dbus-1/system.d/fake-bluez.conf" ''
        <!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
         "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
        <busconfig>
          <policy user="root">
            <allow own="org.bluez"/>
          </policy>
          <policy context="default">
            <allow send_destination="org.bluez"/>
          </policy>
        </busconfig>
      '')
    ];
  };

  testScript = ''
    import os
    API = "http://127.0.0.1:8730"

    def ui_log():
        return machine.succeed("journalctl -u ui.service --no-pager -o cat")

    def fake_log():
        return machine.succeed("cat /tmp/fake-bluez.log")

    def wait_ui(text, timeout=60):
        machine.wait_until_succeeds(f"journalctl -u ui.service --no-pager -o cat | grep -qF '{text}'", timeout=timeout)

    def wait_fake(text, timeout=60):
        machine.wait_until_succeeds(f"grep -qF '{text}' /tmp/fake-bluez.log", timeout=timeout)

    def shot(name):
        machine.screenshot(name)
        # A drawn page is far larger than a blank frame.
        assert os.path.getsize(os.path.join(machine.out_dir, name + ".png")) > 20000, name

    def key(k):
        machine.send_key(k)
        machine.sleep(1)

    machine.wait_for_unit("mediad.service")
    machine.wait_for_file("/dev/dri/card0")
    d = "/media/movies/Alpha (2024)"
    machine.succeed(f"mkdir -p '{d}'")
    machine.succeed(f"ffmpeg -loglevel error -f lavfi -i testsrc=size=320x180:rate=10 -t 5 -c:v libx264 -preset ultrafast '{d}/Alpha (2024).mkv'")
    machine.succeed(f"ffmpeg -loglevel error -f lavfi -i color=c=0x3a6ea5:size=400x600 -frames:v 1 '{d}/poster.jpg'")
    machine.succeed(f"curl -sf -XPOST {API}/rescan")

    machine.succeed("systemd-run --unit=fake-bluez ${python}/bin/python3 ${./test-fake-bluez.py}")
    wait_fake("ready")

    machine.succeed(
        "systemd-run --unit=ui --collect"
        " -E HOME=/root -E XDG_RUNTIME_DIR=/run/user-ui -E LIBSEAT_BACKEND=builtin"
        " -E LIBGL_ALWAYS_SOFTWARE=1 -E MESA_LOADER_DRIVER_OVERRIDE=llvmpipe -E WLR_RENDERER=pixman -E WLR_NO_HARDWARE_CURSORS=1"
        " -E SLINT_BACKEND=winit -E SLINT_SCALE_FACTOR=1 -E UI_HANDOFF=child"
        " -E PATH=/run/current-system/sw/bin"
        " sh -c 'mkdir -p /run/user-ui && chmod 700 /run/user-ui && exec cage -s -- appliance-ui'"
    )
    wait_ui("rows loaded", timeout=90)
    machine.sleep(4)

    with subtest("Settings has a Controllers entry that opens the page and powers the adapter"):
        key("up")     # onto the top bar (opens on the settings button)
        key("ret")    # settings
        wait_ui("settings opened")
        shot("bt-0-settings")
        key("ret")    # Controllers
        wait_ui("controllers page opened")
        wait_fake("Powered True")
        wait_fake("RegisterAgent /org/appliance/agent NoInputNoOutput")
        shot("bt-1-page")

    with subtest("scanning lists both controllers before the other device"):
        key("ret")    # the scan row
        wait_fake("StartDiscovery")
        wait_ui("8BitDo Lite 2 [Available], 8BitDo M30 gamepad [Available], Some Headphones [Available]")
        assert '"scanning":true' in ui_log(), ui_log()
        machine.sleep(2)
        shot("bt-2-scan")

    with subtest("pairing stops the scan, then pairs, trusts and connects"):
        key("down")   # the first device: 8BitDo Lite 2
        key("ret")    # its action list
        shot("bt-3-actions")
        key("ret")    # Pair and connect
        wait_fake("Pair E4:17:D8:00:00:01")
        wait_fake("Trusted E4:17:D8:00:00:01 True")
        wait_fake("Connect E4:17:D8:00:00:01")
        wait_ui("Connected: 8BitDo Lite 2")
        calls = fake_log()
        assert calls.index("StopDiscovery") < calls.index("Pair E4:17:D8:00:00:01"), calls
        machine.sleep(2)
        shot("bt-4-paired")

    with subtest("a connected controller can be forgotten"):
        key("ret")    # its action list: Disconnect, Forget
        key("down")
        key("ret")    # Forget
        wait_fake("RemoveDevice /org/bluez/hci0/dev_E4_17_D8_00_00_01")
        wait_ui("Forgot: 8BitDo Lite 2")
        shot("bt-5-forgot")

    with subtest("back leaves the page and then the settings"):
        key("esc")    # Controllers -> Settings
        wait_ui("controllers page closed")
        key("esc")    # any other button closes Settings
        machine.sleep(2)
        shot("bt-6-home")

    def pad(*actions):
        # A pad that connects after the UI started: created, found by the UI's rescan, then driven.
        machine.succeed("${python}/bin/python3 ${./test-fake-pad.py} " + " ".join(actions))

    def count(text):
        return int(machine.succeed(f"journalctl -u ui.service --no-pager -o cat | grep -cF '{text}' || true").strip())

    with subtest("a game controller that connects later drives the UI: hat, stick and buttons"):
        opened, closed = count("controllers page opened"), count("controllers page closed")
        # D-pad hat up reaches the top bar; A selects Settings; A selects Controllers.
        pad("hat:y:-1", "hat:y:0", "btn:south:1", "btn:south:0", "btn:south:1", "btn:south:0")
        wait_ui("watching an input device")
        assert '"gamepad":true' in ui_log(), ui_log()
        machine.wait_until_succeeds(f"test $(journalctl -u ui.service --no-pager -o cat | grep -cF 'controllers page opened') -gt {opened}", timeout=30)
        shot("bt-7-pad-controllers")
        # The left stick as a D-pad (down), then B goes back: Controllers -> Settings -> closed.
        pad("stick:y:65535", "stick:y:32768", "btn:east:1", "btn:east:0", "btn:east:1", "btn:east:0")
        machine.wait_until_succeeds(f"test $(journalctl -u ui.service --no-pager -o cat | grep -cF 'controllers page closed') -gt {closed}", timeout=30)
        machine.sleep(2)
        shot("bt-8-pad-back")
  '';
}
