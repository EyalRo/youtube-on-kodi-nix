"""Playback, seek and resume against real Kodi. Run with `nix run .#e2e`."""
import os
import shutil
import tempfile
import time
import unittest

import harness

kodi = None
media = None
workdir = None


def setUpModule():
    global kodi, media, workdir
    workdir = tempfile.mkdtemp(prefix="yok-e2e-")
    cache = os.environ.get("YOK_FIXTURES") or os.path.join(workdir, "media")
    os.makedirs(cache, exist_ok=True)
    for variant in ("ts", "fmp4"):
        harness.make_fixture(cache, variant)
    media = harness.serve(cache, harness.MEDIA_PORT)
    kodi = harness.Kodi(workdir)
    harness.serve(os.environ["YOK_MASTER"], int(os.environ["YOK_PORT"]))
    kodi.set_variant("ts")
    kodi.start()


def tearDownModule():
    if kodi:
        kodi.stop()
    harness.stop_servers()
    if workdir and not os.environ.get("YOK_KEEP"):
        shutil.rmtree(workdir, ignore_errors=True)


class PlaybackTest(unittest.TestCase):
    def setUp(self):
        kodi.set_history(0)
        kodi.set_variant("ts")
        kodi.set_engine("inputstream.adaptive")

    def tearDown(self):
        kodi.stop_playback()

    def fail_with_log(self, exc):
        tail = "\n".join(kodi.log().splitlines()[-60:])
        raise AssertionError("%s\n--- kodi.log tail ---\n%s" % (exc, tail))

    def check(self, fn, *a, **kw):
        try:
            return fn(*a, **kw)
        except Exception as e:
            self.fail_with_log(e)

    def test_plays_from_start(self):
        kodi.play()
        self.check(kodi.wait_position, 2, tol=8)

    def test_seek_forward_while_playing(self):
        kodi.play()
        self.check(kodi.wait_position, 2, tol=8)
        kodi.seek(1200)
        self.check(kodi.wait_position, 1200)

    def test_seek_back_and_near_end(self):
        kodi.play()
        self.check(kodi.wait_position, 2, tol=8)
        kodi.seek(1400)
        self.check(kodi.wait_position, 1400)
        kodi.seek(300)
        self.check(kodi.wait_position, 300)

    def test_seek_fmp4_inputstream_adaptive(self):
        kodi.set_variant("fmp4")
        kodi.play()
        self.check(kodi.wait_position, 2, tol=8)
        kodi.seek(1200)
        self.check(kodi.wait_position, 1200)

    @unittest.expectedFailure
    def test_seek_fmp4_kodi_ffmpeg(self):
        """Kodi 21's bundled ffmpeg 6.1 breaks fMP4 HLS after a seek."""
        kodi.set_engine("ffmpeg")
        kodi.set_variant("fmp4")
        kodi.play()
        self.check(kodi.wait_position, 2, tol=8)
        kodi.seek(1200)
        self.check(kodi.wait_position, 1200)

    def test_resume_from_history(self):
        kodi.set_history(1200)
        kodi.play(resume="1")
        self.check(kodi.wait_position, 1200, tol=15)
        # The tracker saves every ~5s; a failed resume used to overwrite 20:00
        # with the position near 0.
        time.sleep(9)
        saved = kodi.history()[harness.VIDEO_ID]["position"]
        self.assertGreaterEqual(saved, 1100, "resume position was overwritten")

    def test_play_from_start_ignores_history(self):
        kodi.set_history(1200)
        kodi.play(fresh="1")
        self.check(kodi.wait_position, 2, tol=8)


if __name__ == "__main__":
    unittest.main()
