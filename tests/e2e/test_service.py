"""The background service refreshes the channel export and games scan by itself.

Run with `nix run .#e2e -- test_service`.
"""
import json
import os
import shutil
import tempfile
import time
import unittest

import harness
import test_games

CHANNEL = {"id": "UCe2eChannel0001", "name": "E2E Channel"}
VIDEOS = [{"id": harness.VIDEO_ID, "title": "A video", "upload_date": "20240602"}]

kodi = None
workdir = None
library = None


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)


def setUpModule():
    try:
        _setup()
    except BaseException:
        tearDownModule()
        raise


def _setup():
    global kodi, workdir, library
    workdir = tempfile.mkdtemp(prefix="yok-e2e-service-")
    library = os.path.join(workdir, "library")
    roms = os.path.join(workdir, "roms")
    os.makedirs(os.path.join(roms, "NES"))
    test_games.make_nes_rom(os.path.join(roms, "NES", "Test Game (World).nes"))

    def seed(profile):
        cache = os.path.join(profile, "cache")
        write(os.path.join(profile, "channels.conf"), "@e2e | %s\n" % CHANNEL["name"])
        write(os.path.join(cache, "channels.json"), json.dumps({"@e2e": dict(CHANNEL, timestamp=time.time())}))
        write(os.path.join(cache, CHANNEL["id"] + ".json"),
              json.dumps({"end": 50, "videos": VIDEOS, "timestamp": time.time(), "retry_after": 0}))
        write(os.path.join(profile, "settings.xml"),
              '<settings version="2"><setting id="library_dir">%s</setting>'
              '<setting id="games_dir">%s</setting>'
              '<setting id="media_refresh_minutes">1</setting></settings>' % (library, roms))

    kodi = harness.Kodi(workdir)
    kodi.start(seed=seed)


def tearDownModule():
    if kodi:
        kodi.stop()
    harness.stop_servers()
    if workdir and not os.environ.get("YOK_KEEP"):
        shutil.rmtree(workdir, ignore_errors=True)


class ServiceTest(unittest.TestCase):
    def test_service_exports_channels_and_scans_games_unprompted(self):
        # First pass runs ~20s after login; nothing here calls the plugin.
        try:
            kodi._wait(lambda: os.path.exists(os.path.join(kodi.profile, "games.json")), 90, "games scan")
            kodi._wait(lambda: os.path.isdir(library) and any(
                f.endswith(".strm") for _, _, fs in os.walk(library) for f in fs), 30, "channel export")
        except Exception as e:
            tail = "\n".join(l for l in kodi.log().splitlines() if "plugin.video" in l)[-3000:]
            raise AssertionError("%s\n--- add-on log ---\n%s" % (e, tail))
        log = kodi.log()
        self.assertIn("refresh service started", log)
        self.assertIn("library export done: 1 channel(s)", log)
        self.assertIn("scanned ", log)


if __name__ == "__main__":
    unittest.main()
