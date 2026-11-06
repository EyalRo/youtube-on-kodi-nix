# NixOS VM test for the appliance module: mediad under its systemd hardening, with a tmpfs
# standing in for the NFS share and a fixture that drops one downloaded video into it.
{ pkgs, appliance, module }:

let
  # Writes what yt-dlp leaves for one video (video, thumbnail, `.info.json`, channel poster) in
  # the given folder, the way a downloader outside this appliance (MediaWatch) delivers it.
  placeVideo = pkgs.writeShellScript "place-video" ''
    dir="$1"
    mkdir -p "$dir"
    stem="$dir/2024-06-02 - Fake Upload [abc123XYZ]"
    cat > "$stem.info.json" <<JSON
    {"id":"abc123XYZ","title":"Fake Upload","channel":"Test Channel","upload_date":"20240602"}
    JSON
    printf 'jpg' > "$stem.jpg"
    printf 'mkv' > "$stem.mkv"
    printf avatar > "$dir/poster.jpg"
  '';
in
pkgs.testers.runNixOSTest {
  name = "media-appliance";

  nodes.machine = { pkgs, ... }: {
    imports = [ module ];

    # Stand-in for the NFS share: a separate mount, as in production.
    systemd.mounts = [{
      what = "tmpfs"; where = "/media"; type = "tmpfs"; options = "mode=0755";
      wantedBy = [ "local-fs.target" ]; before = [ "local-fs.target" ];
    }];
    systemd.tmpfiles.rules = [ "d /media/youtube 0755 root root -" ];

    services.media-appliance = {
      enable = true;
      package = appliance;
      rescanSeconds = 0;
    };
  };

  testScript = ''
    machine.wait_for_unit("media.mount")
    machine.wait_for_unit("mediad.service")
    machine.wait_until_succeeds("curl -sf http://127.0.0.1:8730/health")

    with subtest("mediad is ordered after the media mount"):
        after = machine.succeed("systemctl show mediad.service -p After --value")
        assert "media.mount" in after, after

    with subtest("hardening is in effect"):
        assert "strict" in machine.succeed("systemctl show mediad.service -p ProtectSystem")
        # mediad only reads the library.
        machine.fail("sudo -u mediad touch /media/x")
        ro = machine.succeed("systemctl show mediad.service -p ReadOnlyPaths")
        assert "/media" in ro, ro

    with subtest("library starts empty"):
        assert "Test Channel" not in machine.succeed("curl -sf http://127.0.0.1:8730/rows/everything")

    with subtest("a downloaded video is placed and a rescan is requested"):
        machine.succeed("${placeVideo} '/media/youtube/Test Channel'")
        machine.succeed("test -f '/media/youtube/Test Channel/2024-06-02 - Fake Upload [abc123XYZ].mkv'")
        machine.succeed("test -f '/media/youtube/Test Channel/2024-06-02 - Fake Upload [abc123XYZ].info.json'")
        machine.succeed("curl -sf -X POST http://127.0.0.1:8730/rescan")

    with subtest("mediad serves the video after the rescan request"):
        import json
        for row in ["channels", "everything"]:
            machine.wait_until_succeeds(f"curl -sf http://127.0.0.1:8730/rows/{row} | grep -q 'Test Channel'", timeout=30)
            items = json.loads(machine.succeed(f"curl -sf http://127.0.0.1:8730/rows/{row}"))["items"]
            assert len(items) == 1, items
            assert items[0]["kind"] == "channel" and items[0]["date"] == "2024-06-02", items
            assert items[0]["date_source"] == "info_json", items
        assert json.loads(machine.succeed("curl -sf http://127.0.0.1:8730/rows/channels"))["items"][0]["art"].endswith("poster.jpg")
        machine.succeed(f"curl -sf http://127.0.0.1:8730/items/{items[0]['id']}/art | grep -q avatar")
        detail = json.loads(machine.succeed(f"curl -sf http://127.0.0.1:8730/items/{items[0]['id']}"))
        eps = detail["episodes"]
        assert len(eps) == 1 and eps[0]["title"] == "Fake Upload" and eps[0]["aired"] == "2024-06-02", eps

    with subtest("the service user cannot write to the library"):
        # Writable at the filesystem level, so only the unit sandbox can stop the write.
        machine.succeed("chown mediad:mediad /media/youtube")
        machine.succeed("sudo -u mediad touch /media/youtube/plain-write-ok && rm /media/youtube/plain-write-ok")
        machine.fail("systemd-run --wait -p User=mediad -p ProtectSystem=strict -p ReadOnlyPaths=/media touch /media/youtube/x")
  '';
}
