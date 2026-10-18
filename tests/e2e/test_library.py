"""Exported YouTube channels as Kodi library TV shows: export, scan, play.

Run with `nix run .#e2e -- test_library`.
"""
import json
import os
import shutil
import tempfile
import time
import unittest

import harness

CHANNEL = {"id": "UCe2eChannel0001", "name": "E2E Channel"}
VIDEOS = [  # newest first, as in a channel listing
    {"id": harness.VIDEO_ID, "title": "Newest video", "upload_date": "20240602"},
    {"id": "e2eVideo002", "title": "Middle video", "upload_date": "20240301"},
    {"id": "e2eVideo003", "title": "Old video", "upload_date": "20231215"},
]

kodi = None
workdir = None
library = None


def seed_addon_profile():
    """Pre-fill the add-on's caches so the export needs no network."""
    cache = os.path.join(kodi.profile, "cache")
    os.makedirs(cache, exist_ok=True)
    with open(os.path.join(kodi.profile, "channels.conf"), "w") as f:
        f.write("@e2e | %s\n" % CHANNEL["name"])
    with open(os.path.join(cache, "channels.json"), "w") as f:
        json.dump({"@e2e": dict(CHANNEL, timestamp=time.time())}, f)
    with open(os.path.join(cache, CHANNEL["id"] + ".json"), "w") as f:
        json.dump({"end": 50, "videos": VIDEOS, "timestamp": time.time(), "retry_after": 0}, f)
    with open(os.path.join(kodi.profile, "settings.xml"), "w") as f:
        f.write('<settings version="2"><setting id="library_dir">%s</setting>'
                '<setting id="playback_engine">inputstream.adaptive</setting></settings>' % library)


def setUpModule():
    try:
        _setup()
    except BaseException:
        tearDownModule()  # don't leave Kodi holding the RPC port
        raise


def _setup():
    global kodi, workdir, library
    workdir = tempfile.mkdtemp(prefix="yok-e2e-lib-")
    library = os.path.join(workdir, "library")
    os.makedirs(library)
    cache = os.environ.get("YOK_FIXTURES") or os.path.join(workdir, "media")
    os.makedirs(cache, exist_ok=True)
    harness.make_fixture(cache, "ts")
    harness.serve(cache, harness.MEDIA_PORT)
    kodi = harness.Kodi(workdir)
    harness.serve(os.environ["YOK_MASTER"], int(os.environ["YOK_PORT"]))
    kodi.set_variant("ts")
    kodi.start()  # creates the video database
    seed_addon_profile()
    kodi.make_tvshow_source(library)
    kodi.rpc("Addons.ExecuteAddon", addonid=harness.ADDON_ID, params={"mode": "export_library"})
    kodi._wait(lambda: kodi.rpc("VideoLibrary.GetTVShows")["limits"]["total"] >= 1, 60, "library scan")
    kodi._wait(lambda: kodi.rpc("VideoLibrary.GetEpisodes")["limits"]["total"] >= len(VIDEOS), 60, "episodes")


def tearDownModule():
    if kodi:
        kodi.stop()
    harness.stop_servers()
    if workdir and not os.environ.get("YOK_KEEP"):
        shutil.rmtree(workdir, ignore_errors=True)


class LibraryTest(unittest.TestCase):
    def tearDown(self):
        kodi.stop_playback()

    def fail_with_log(self, exc):
        tail = "\n".join(kodi.log().splitlines()[-60:])
        raise AssertionError("%s\n--- kodi.log tail ---\n%s" % (exc, tail))

    def episodes(self):
        return kodi.rpc("VideoLibrary.GetEpisodes", properties=[
            "title", "showtitle", "season", "episode", "firstaired", "file", "playcount", "tvshowid"],
            sort={"method": "episode", "order": "descending"})["episodes"]

    def test_channel_is_a_tagged_tv_show(self):
        (show,) = kodi.rpc("VideoLibrary.GetTVShows", properties=["title", "tag"])["tvshows"]
        self.assertEqual(show["title"], CHANNEL["name"])
        self.assertIn("YouTube", show["tag"])

    def test_episodes_have_dates_and_seasons(self):
        eps = {e["title"]: e for e in self.episodes()}
        self.assertEqual(len(eps), len(VIDEOS))
        self.assertEqual(eps["Newest video"]["firstaired"], "2024-06-02")
        self.assertEqual(eps["Newest video"]["season"], 2024)
        self.assertEqual(eps["Old video"]["season"], 2023)

    def test_library_episode_plays_through_strm(self):
        ep = next(e for e in self.episodes() if e["title"] == "Newest video")
        # The forum-reported risk: the library keeping something other than the
        # playable plugin:// path. Log what it stores either way.
        print("episode file stored as: %r" % ep["file"])
        kodi.rpc("Player.Open", item={"episodeid": ep["episodeid"]})
        try:
            kodi.wait_position(2, tol=8)
        except Exception as e:
            self.fail_with_log(e)
        item = kodi.rpc("Player.GetItem", playerid=kodi.player(), properties=["title", "showtitle", "episode"])["item"]
        self.assertEqual(item.get("showtitle"), CHANNEL["name"])


if __name__ == "__main__":
    unittest.main()
