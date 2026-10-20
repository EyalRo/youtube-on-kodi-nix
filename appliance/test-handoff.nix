# NixOS VM test for the display handoff: a stand-in "UI" process holds DRM master on a
# virtio-gpu display, drops it, `play` runs the real mpv on the bare display, and the
# stand-in takes master back. Software decode only; the NUC's VA-API/HDR are hardware gates.
{ pkgs, appliance, module }:

let
  # Holds DRM master like the UI would. SIGUSR1 drops it, SIGUSR2 takes it back; every
  # transition is logged with the ioctl result so the test can assert on it.
  holder = pkgs.writers.writePython3Bin "drm-holder" { flakeIgnore = [ "E501" ]; } ''
    import fcntl
    import os
    import signal
    import time

    DROP, SET = 0x641F, 0x641E  # _IO('d', 0x1f), _IO('d', 0x1e)
    fd = os.open("/dev/dri/card0", os.O_RDWR)


    def ioctl(req):
        try:
            fcntl.ioctl(fd, req)
            return "ok"
        except OSError as e:
            return f"errno={e.errno}"


    def log(msg):
        print(msg, flush=True)


    log("holding: set_master=" + ioctl(SET))
    signal.signal(signal.SIGUSR1, lambda *_: log("dropped: " + ioctl(DROP)))
    signal.signal(signal.SIGUSR2, lambda *_: log("retaken: " + ioctl(SET)))
    while True:
        time.sleep(0.1)
  '';
in
pkgs.testers.runNixOSTest {
  name = "media-appliance-handoff";

  nodes.machine = { pkgs, ... }: {
    imports = [ module ];
    boot.initrd.kernelModules = [ "virtio_gpu" ];
    virtualisation.qemu.options = [ "-device virtio-gpu-pci" ];
    environment.systemPackages = [ appliance holder pkgs.mpv pkgs.ffmpeg pkgs.jq pkgs.sqlite ];
    systemd.tmpfiles.rules = [ "d /media/movies 0755 root root -" ];
    services.media-appliance = { enable = true; package = appliance; rescanSeconds = 0; };
    # Software video output straight to the display (no GL needed); the TV's connector
    # name is a VM one here.
    environment.variables = {
      PLAYER_MPV_ARGS = "--vo=drm --drm-device=/dev/dri/card0 --ao=null --hwdec=no";
      PLAYER_DRM_CONNECTOR = "Virtual-1";
    };
  };

  testScript = ''
    import json
    API = "http://127.0.0.1:8730"

    def api(path):
        return json.loads(machine.succeed(f"curl -sf {API}{path}"))

    def holder_log():
        return machine.succeed("cat /tmp/holder.log")

    def wait_log(text):
        machine.wait_until_succeeds(f"grep -q '{text}' /tmp/holder.log", timeout=30)

    def watched(i):
        return machine.succeed(f"sqlite3 /persist/mediad/mediad.sqlite \"select watched from play_state where id='{i}'\"").strip()

    machine.wait_for_unit("mediad.service")
    machine.wait_for_file("/dev/dri/card0")
    print(machine.succeed("ls -l /dev/dri; for c in /sys/class/drm/card0-*; do echo $c $(cat $c/status); done"))

    machine.succeed("mkdir -p '/media/movies/Clip (2024)' && ffmpeg -loglevel error -f lavfi -i testsrc=size=640x360:rate=10 -t 4 -c:v libx264 -preset ultrafast '/media/movies/Clip (2024)/Clip (2024).mkv'")
    machine.succeed(f"curl -sf -XPOST {API}/rescan")
    clip = next(i["id"] for i in api("/rows/movies")["items"] if i["title"].startswith("Clip"))

    machine.succeed("(drm-holder >/tmp/holder.log 2>&1 &)")
    wait_log("holding: set_master=ok")

    with subtest("while the UI holds master, mpv cannot take the display"):
        why = machine.execute("mpv --vo=drm --drm-device=/dev/dri/card0 --drm-connector=Virtual-1 --ao=null --hwdec=no --length=1 /media/movies/Clip*/*.mkv 2>&1")[1]
        assert "Failed to acquire DRM master" in why, why
        out = machine.execute(f"timeout 20 play {clip} 2>&1")
        print(out)
        assert out[0] != 0, "play unexpectedly succeeded while another process held DRM master"
        assert watched(clip) == "", "a failed handoff must not mark the item watched"

    for n in range(1, 4):
        with subtest(f"handoff cycle {n}: drop, play on the bare display, take back"):
            machine.succeed("pkill -USR1 -f drm-holder")
            wait_log("dropped: ok")
            machine.succeed(f"timeout 60 play {clip}")
            machine.succeed("pkill -USR2 -f drm-holder")
            machine.wait_until_succeeds("test $(grep -c 'retaken: ok' /tmp/holder.log) -ge " + str(n), timeout=30)
            # Ready for the next cycle: a count-based wait on "dropped".
            machine.succeed("test $(grep -c 'dropped: ok' /tmp/holder.log) -eq " + str(n))
        assert watched(clip) == "1"
        machine.succeed("sqlite3 /persist/mediad/mediad.sqlite \"update play_state set watched=0 where id='" + clip + "'\"")

    print(holder_log())
  '';
}
