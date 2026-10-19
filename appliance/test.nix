# NixOS VM test for the appliance module: mediad + ytsync under their systemd
# hardening, with a tmpfs standing in for the NFS share and a fake yt-dlp.
{ pkgs, appliance, module }:

let
  # Writes what the real yt-dlp writes for one video, using the paths ytsync gives it
  # (`-o <work dir>/<template>`, `--download-archive <file>`).
  fakeYtDlp = pkgs.writeShellScriptBin "yt-dlp" ''
    while [ $# -gt 0 ]; do
      case "$1" in
        -o) out=$2; shift ;;
        --download-archive) archive=$2; shift ;;
      esac
      shift
    done
    stem="$(dirname "$out")/2024-06-02 - Fake Upload [abc123XYZ]"
    cat > "$stem.info.json" <<JSON
    {"id":"abc123XYZ","title":"Fake Upload","channel":"Test Channel","upload_date":"20240602"}
    JSON
    printf 'jpg' > "$stem.jpg"
    printf 'mkv' > "$stem.mkv"
    echo "youtube abc123XYZ" >> "$archive"
  '';
in
pkgs.testers.runNixOSTest {
  name = "media-appliance";
  node.pkgsReadOnly = false;  # lets the node overlay yt-dlp

  nodes.machine = { pkgs, ... }: {
    imports = [ module ];

    nixpkgs.overlays = [ (_: _: { yt-dlp = fakeYtDlp; }) ];

    # Stand-in for the NFS share: a separate mount, as in production.
    systemd.mounts = [{
      what = "tmpfs"; where = "/media"; type = "tmpfs"; options = "mode=0755";
      wantedBy = [ "local-fs.target" ]; before = [ "local-fs.target" ];
    }];
    # The share's youtube folder exists and belongs to the service user.
    systemd.tmpfiles.rules = [ "d /media/youtube 0755 mediad mediad -" ];

    services.media-appliance = {
      enable = true;
      package = appliance;
      rescanSeconds = 0;
      ytsync = {
        enable = true;
        channelsFile = pkgs.writeText "channels.conf" "@testchannel | Test Channel\n";
      };
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
        assert "strict" in machine.succeed("systemctl show ytsync.service -p ProtectSystem")
        rw = machine.succeed("systemctl show ytsync.service -p ReadWritePaths")
        assert "/media/youtube" in rw, rw
        # mediad only reads the library.
        machine.fail("sudo -u mediad touch /media/x")

    with subtest("library starts empty"):
        assert "Test Channel" not in machine.succeed("curl -sf http://127.0.0.1:8730/rows/everything")

    with subtest("ytsync delivers through the hardened unit"):
        machine.succeed("systemctl start ytsync.service")
        machine.succeed("test -f '/media/youtube/Test Channel/2024-06-02 - Fake Upload [abc123XYZ].mkv'")
        machine.succeed("test -f '/media/youtube/Test Channel/2024-06-02 - Fake Upload [abc123XYZ].info.json'")
        machine.succeed("grep -q abc123XYZ /persist/ytsync/archive.txt")

    with subtest("mediad serves the video after ytsync's rescan request"):
        import json
        for row in ["channels", "everything"]:
            machine.wait_until_succeeds(f"curl -sf http://127.0.0.1:8730/rows/{row} | grep -q 'Test Channel'", timeout=30)
            items = json.loads(machine.succeed(f"curl -sf http://127.0.0.1:8730/rows/{row}"))["items"]
            assert len(items) == 1, items
            assert items[0]["kind"] == "channel" and items[0]["date"] == "2024-06-02", items
            assert items[0]["date_source"] == "info_json", items
        detail = json.loads(machine.succeed(f"curl -sf http://127.0.0.1:8730/items/{items[0]['id']}"))
        eps = detail["episodes"]
        assert len(eps) == 1 and eps[0]["title"] == "Fake Upload" and eps[0]["aired"] == "2024-06-02", eps

    with subtest("the service user cannot write elsewhere in the library"):
        machine.fail("systemd-run --wait -p User=mediad -p ProtectSystem=strict -p ReadWritePaths=/media/youtube touch /media/movies-x")
  '';
}
