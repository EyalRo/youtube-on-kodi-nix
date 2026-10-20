# NixOS VM test for `play` with a real mpv (null video/audio outputs: the VM has no TV).
# Covers what the unit tests can only fake: resume (--start), progress posts to mediad,
# end-of-file marking the item watched, and quitting early not erasing a resume point.
{ pkgs, appliance, module }:

pkgs.testers.runNixOSTest {
  name = "media-appliance-player";

  nodes.machine = { pkgs, ... }: {
    imports = [ module ];
    environment.systemPackages = [ appliance pkgs.mpv pkgs.ffmpeg pkgs.socat pkgs.jq pkgs.sqlite ];
    systemd.tmpfiles.rules = [ "d /media/movies 0755 root root -" ];
    services.media-appliance = {
      enable = true;
      package = appliance;
      rescanSeconds = 0;
    };
    environment.variables = {
      PLAYER_MPV_ARGS = "--vo=null --ao=null --hwdec=no";
    };
  };

  testScript = ''
    import json
    API = "http://127.0.0.1:8730"

    def api(path):
        return json.loads(machine.succeed(f"curl -sf {API}{path}"))

    def movie(title, seconds):
        d = f"/media/movies/{title} (2024)"
        machine.succeed(f"mkdir -p '{d}' && ffmpeg -loglevel error -f lavfi -i testsrc=size=160x120:rate=5 -t {seconds} -c:v libx264 -preset ultrafast '{d}/{title} (2024).mkv'")

    def item_id(title):
        for i in api("/rows/movies")["items"]:
            if i["title"].startswith(title):
                return i["id"]
        raise AssertionError(title)

    def progress(i, pos, dur):
        machine.succeed(f"curl -sf -XPOST -H 'content-type: application/json' -d '{{\"position\":{pos},\"duration\":{dur}}}' {API}/items/{i}/progress")

    def start_play(i, extra=""):
        machine.succeed("rm -f /tmp/play-*.sock")
        machine.succeed(f"(PLAYER_MPV_ARGS='--vo=null --ao=null --hwdec=no {extra}' play {i} >/tmp/play.log 2>&1 &)")
        machine.wait_until_succeeds("ls /tmp/play-*.sock", timeout=30)
        return machine.succeed("ls /tmp/play-*.sock").strip()

    def mpv(sock, *cmd):
        return machine.succeed(f"echo '{json.dumps({'command': list(cmd)})}' | socat - UNIX-CONNECT:{sock}")

    def stopped():
        machine.wait_until_fails("pgrep -x play", timeout=30)

    machine.wait_for_unit("mediad.service")
    machine.wait_until_succeeds(f"curl -sf {API}/health")
    movie("Long", 300)
    movie("Short", 8)
    machine.succeed(f"curl -sf -XPOST {API}/rescan")
    long_id, short_id = item_id("Long"), item_id("Short")

    with subtest("a saved position is resumed by mpv (--start)"):
        progress(long_id, 100, 300)
        assert api(f"/items/{long_id}/play")["resume_position"] == 100
        sock = start_play(long_id, "--pause")
        machine.wait_until_succeeds(f"echo '{{\"command\":[\"get_property\",\"time-pos\"]}}' | socat - UNIX-CONNECT:{sock} | grep -q '\"data\":100'", timeout=30)
        mpv(sock, "quit")
        stopped()
        assert api(f"/items/{long_id}/play")["resume_position"] == 100

    with subtest("progress is posted while playing"):
        sock = start_play(long_id)
        machine.wait_until_succeeds(f"curl -sf {API}/items/{long_id}/play | jq -e '.resume_position > 100.5'", timeout=60)
        mpv(sock, "quit")
        stopped()
        pos = api(f"/items/{long_id}/play")["resume_position"]
        assert 100.5 < pos < 200, pos

    with subtest("quitting at once does not erase the resume point"):
        progress(long_id, 100, 300)
        sock = start_play(long_id)
        mpv(sock, "quit")
        stopped()
        pos = api(f"/items/{long_id}/play")["resume_position"]
        assert pos >= 100, pos

    with subtest("playing to the end marks the item watched"):
        machine.succeed(f"timeout 60 play {short_id}")
        # mediad does not expose watched for movies (Item.watched counts episodes), so read the store.
        state = machine.succeed(f"sqlite3 /persist/mediad/mediad.sqlite \"select watched, position from play_state where id='{short_id}'\"").strip()
        assert state == "1|0.0", state
        assert api(f"/items/{short_id}/play")["resume_position"] == 0
  '';
}
