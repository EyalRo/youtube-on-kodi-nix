"""Tests for the channel -> Kodi library exporter (pure Python)."""
import os
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "addon", "resources", "lib"))
import libexport  # noqa: E402

CH = {"id": "UC123", "name": "Some: Channel/Name"}


def vid(i, published=None, upload_date=None, title=None):
    return {"id": i, "title": title or "Title " + i, "thumb": "http://t/" + i,
            "published": published, "upload_date": upload_date}


class ExportTest(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp()
        self.logs = []
        self.log = lambda lvl, msg: self.logs.append((lvl, msg))

    def folder(self):
        (name,) = os.listdir(self.root)
        return os.path.join(self.root, name)

    def test_layout_and_nfo(self):
        libexport.export_channel(self.root, CH, [vid("a", "2024-03-05T10:00:00+00:00")], log=self.log)
        self.assertEqual(os.path.basename(self.folder()), "Some Channel Name [UC123]")
        files = sorted(os.listdir(self.folder()))
        self.assertEqual(files, ["2024-03-05 - Title a [a].nfo", "2024-03-05 - Title a [a].strm", "tvshow.nfo"])
        show = ET.parse(os.path.join(self.folder(), "tvshow.nfo")).getroot()
        self.assertEqual(show.find("tag").text, "YouTube")
        ep = ET.parse(os.path.join(self.folder(), "2024-03-05 - Title a [a].nfo")).getroot()
        self.assertEqual(ep.find("season").text, "2024")
        self.assertEqual(ep.find("aired").text, "2024-03-05")
        self.assertEqual(ep.find("episode").text, str(65 * 100))
        with open(os.path.join(self.folder(), "2024-03-05 - Title a [a].strm")) as f:
            self.assertTrue(f.read().startswith("plugin://plugin.video.myyoutubers/?mode=play&id=a&"))

    def test_episode_numbers_stable_under_backfill(self):
        newer = vid("n", "2024-06-01T00:00:00Z")
        libexport.export_channel(self.root, CH, [newer], log=self.log)
        libexport.export_channel(self.root, CH, [newer, vid("o", "2024-01-02T00:00:00Z")], log=self.log)
        ep = ET.parse(os.path.join(self.folder(), "2024-06-01 - Title n [n].nfo")).getroot()
        # day-of-year of 2024-06-01 is 153 (leap year)
        self.assertEqual(int(ep.find("episode").text), 15300)

    def test_same_day_uploads_get_distinct_numbers(self):
        libexport.export_channel(
            self.root, CH, [vid("a", "2024-03-05T01:00:00Z"), vid("b", "2024-03-05T02:00:00Z")], log=self.log)
        nums = sorted(int(ET.parse(os.path.join(self.folder(), f)).getroot().find("episode").text)
                      for f in os.listdir(self.folder()) if f.endswith(".nfo") and f != "tvshow.nfo")
        self.assertEqual(nums, [6500, 6501])

    def test_cap_keeps_newest_and_undated_are_logged(self):
        videos = [vid("a", "2024-01-01T00:00:00Z"), vid("b", "2024-02-01T00:00:00Z"),
                  vid("c", upload_date="20240301"), vid("d")]
        libexport.export_channel(self.root, CH, videos, max_videos=2, log=self.log)
        strms = sorted(f for f in os.listdir(self.folder()) if f.endswith(".strm"))
        self.assertEqual([s[:10] for s in strms], ["2024-02-01", "2024-03-01"])
        self.assertIn("WARN", [lvl for lvl, _ in self.logs])

    def test_rerun_is_idempotent_and_never_deletes(self):
        libexport.export_channel(self.root, CH, [vid("a", "2024-01-01T00:00:00Z")], log=self.log)
        self.assertEqual(libexport.export_channel(self.root, CH, [vid("a", "2024-01-01T00:00:00Z")], log=self.log), 0)
        libexport.export_channel(self.root, CH, [vid("b", "2024-02-01T00:00:00Z")], max_videos=1, log=self.log)
        self.assertEqual(len([f for f in os.listdir(self.folder()) if f.endswith(".strm")]), 2)

    def test_xml_escaping(self):
        libexport.export_channel(self.root, CH, [vid("a", "2024-01-01T00:00:00Z", title="Tom & <Jerry>")], log=self.log)
        nfo = [f for f in os.listdir(self.folder()) if f.endswith(".nfo") and f != "tvshow.nfo"][0]
        ep = ET.parse(os.path.join(self.folder(), nfo)).getroot()
        self.assertEqual(ep.find("title").text, "Tom & <Jerry>")


if __name__ == "__main__":
    unittest.main()
