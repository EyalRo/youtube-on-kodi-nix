"""Home-screen rows over a mixed library: YouTube channel, TV show, movie, game.

Run with `nix run .#e2e -- test_rows`.
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
VIDEOS = [
    {"id": harness.VIDEO_ID, "title": "Newest video", "upload_date": "20240602"},
    {"id": "e2eVideo002", "title": "Older video", "upload_date": "20240301"},
]

kodi = None
workdir = None


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
    global kodi, workdir
    workdir = tempfile.mkdtemp(prefix="yok-e2e-rows-")
    library = os.path.join(workdir, "library")  # TV shows: channel export + a plain show
    movies = os.path.join(workdir, "movies")
    roms = os.path.join(workdir, "roms")
    stream = "http://127.0.0.1:%d/ts/video.m3u8" % harness.MEDIA_PORT
    # Plain TV show, no YouTube tag.
    show = os.path.join(library, "Plain Show")
    write(os.path.join(show, "tvshow.nfo"), "<tvshow><title>Plain Show</title></tvshow>")
    write(os.path.join(show, "S01E01 - Pilot.strm"), stream)
    write(os.path.join(show, "S01E01 - Pilot.nfo"),
          "<episodedetails><title>Pilot</title><season>1</season><episode>1</episode>"
          "<aired>2010-01-01</aired></episodedetails>")
    # Movie.
    write(os.path.join(movies, "Test Movie (1999).strm"), stream)
    write(os.path.join(movies, "Test Movie (1999).nfo"),
          "<movie><title>Test Movie</title><premiered>1999-03-31</premiered></movie>")
    # Game.
    os.makedirs(os.path.join(roms, "NES"))
    test_games.make_nes_rom(os.path.join(roms, "NES", "Test Game (World).nes"))
    write(os.path.join(roms, "NES", "Test Game (World).json"), json.dumps({"released": "1987-05"}))

    kodi = harness.Kodi(workdir)
    kodi.start()
    cache = os.path.join(kodi.profile, "cache")
    write(os.path.join(kodi.profile, "channels.conf"), "@e2e | %s\n" % CHANNEL["name"])
    write(os.path.join(cache, "channels.json"), json.dumps({"@e2e": dict(CHANNEL, timestamp=time.time())}))
    write(os.path.join(cache, CHANNEL["id"] + ".json"),
          json.dumps({"end": 50, "videos": VIDEOS, "timestamp": time.time(), "retry_after": 0}))
    write(os.path.join(kodi.profile, "settings.xml"),
          '<settings version="2"><setting id="library_dir">%s</setting>'
          '<setting id="games_dir">%s</setting></settings>' % (library, roms))
    kodi.make_sources({library: "tvshows", movies: "movies"})
    run = lambda **p: kodi.rpc("Addons.ExecuteAddon", addonid=harness.ADDON_ID, params=p)
    # One plugin run at a time: a second ExecuteAddon pre-empts the first.
    run(mode="export_library")  # also scans `library`, plain show included
    kodi._wait(lambda: any(d.startswith(CHANNEL["name"]) for d in os.listdir(library)), 30, "channel export")
    time.sleep(3)
    run(mode="scan_games")
    kodi._wait(lambda: os.path.exists(os.path.join(kodi.profile, "games.json")), 30, "games.json")
    kodi.rpc("VideoLibrary.Scan", directory=movies)
    count = lambda m, k: kodi.rpc(m)["limits"]["total"]
    kodi._wait(lambda: count("VideoLibrary.GetTVShows", "tvshows") >= 2, 90, "TV shows")
    kodi._wait(lambda: count("VideoLibrary.GetEpisodes", "episodes") >= 3, 60, "episodes")
    kodi._wait(lambda: count("VideoLibrary.GetMovies", "movies") >= 1, 60, "movie")
    # Watch one channel video: the channel now has progress and an unwatched episode.
    older = next(e for e in kodi.rpc("VideoLibrary.GetEpisodes", properties=["title"])["episodes"]
                 if e["title"] == "Older video")
    kodi.rpc("VideoLibrary.SetEpisodeDetails", episodeid=older["episodeid"], playcount=1)


def tearDownModule():
    if kodi:
        kodi.stop()
    harness.stop_servers()
    if workdir and not os.environ.get("YOK_KEEP"):
        shutil.rmtree(workdir, ignore_errors=True)


def directory(**params):
    url = "plugin://%s/?" % harness.ADDON_ID + "&".join("%s=%s" % kv for kv in params.items())
    files = kodi.rpc("Files.GetDirectory", directory=url, media="video").get("files", [])
    return [f["label"] for f in files]


class RowsTest(unittest.TestCase):
    def fail_with_log(self, exc):
        tail = "\n".join(kodi.log().splitlines()[-60:])
        raise AssertionError("%s\n--- kodi.log tail ---\n%s" % (exc, tail))

    def check(self, row, expected):
        try:
            got = directory(mode="row", id=row)
        except Exception as e:
            self.fail_with_log(e)
        self.assertEqual(got, expected, row)

    def test_row_list(self):
        self.assertEqual(directory(mode="rows"), [
            "Everything (4)", "Movies (1)", "TV shows (1)", "YouTube channels (1)",
            "Shows with unwatched episodes (1)", "Games (1)", "NES (1)"])

    def test_everything_mixes_all_content_newest_first(self):
        self.check("everything", ["E2E Channel", "Plain Show", "Test Movie", "Test Game"])

    def test_content_type_rows(self):
        self.check("movies", ["Test Movie"])
        self.check("tvshows", ["Plain Show"])
        self.check("channels", ["E2E Channel"])
        self.check("games", ["Test Game"])
        self.check("games:nes", ["Test Game"])

    def test_shows_with_unwatched_episodes(self):
        self.check("unwatched", ["E2E Channel"])


if __name__ == "__main__":
    unittest.main()
