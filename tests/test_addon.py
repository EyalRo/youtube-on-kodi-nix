"""Smoke tests for the add-on with Kodi's Python modules stubbed out.

Run: python3 -m unittest discover -s tests
"""
import importlib
import os
import stat
import sys
import tempfile
import types
import unittest
from unittest import mock

ADDON = os.path.join(os.path.dirname(__file__), "..", "addon")
RESOLVED = []


class _ListItem:
    def __init__(self, label="", label2="", path=""):
        self.label, self.label2, self.path, self.props, self.menu = label, label2, path, {}, []

    def setInfo(self, *a): pass
    def setArt(self, *a): pass
    def setProperty(self, k, v): self.props[k] = v
    def addContextMenuItems(self, items): self.menu = items
    def setMimeType(self, *a): pass
    def setContentLookup(self, *a): pass


def _stub_kodi(profile):
    addon = mock.Mock()
    addon.getAddonInfo.side_effect = lambda k: {"path": ADDON, "profile": profile, "id": "plugin.video.myyoutubers"}[k]
    addon.getSetting.return_value = ""
    mods = {
        "xbmc": mock.MagicMock(),
        "xbmcaddon": types.SimpleNamespace(Addon=lambda: addon),
        "xbmcgui": mock.MagicMock(ListItem=_ListItem),
        "xbmcplugin": mock.MagicMock(),
        "xbmcvfs": types.SimpleNamespace(translatePath=lambda p: p),
    }
    mods["xbmcplugin"].setResolvedUrl.side_effect = lambda h, ok, li: RESOLVED.append((ok, li))
    sys.modules.update(mods)


def _fake_ytdlp(path, body):
    with open(path, "w") as f:
        f.write("#!/bin/sh\n" + body)
    os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)


class AddonTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.master = os.path.join(self.tmp, "master")
        os.makedirs(self.master)
        _stub_kodi(os.path.join(self.tmp, "profile"))
        sys.path.insert(0, os.path.join(ADDON, "resources", "lib"))
        sys.modules.pop("kanyt", None)
        self.kanyt = importlib.import_module("kanyt")
        self.kanyt._YTDLP_EXEC = os.path.join(self.tmp, "yt-dlp")
        self.kanyt._MASTER_DIR = self.master
        self.kanyt._MASTER_PORT = "8099"

    def test_hls_pair_becomes_master_playlist(self):
        _fake_ytdlp(self.kanyt._YTDLP_EXEC, 'printf "https://v/video.m3u8\\nhttps://a/audio.m3u8\\n"\n')
        url = self.kanyt.get_stream_url("abc")
        self.assertEqual(url, "http://127.0.0.1:8099/abc.m3u8")
        with open(os.path.join(self.master, "abc.m3u8")) as f:
            text = f.read()
        self.assertIn('URI="https://a/audio.m3u8"', text)
        self.assertIn("https://v/video.m3u8", text)

    def test_falls_back_to_muxed(self):
        # First call (HLS pair) yields one line; the muxed lookup reuses it.
        _fake_ytdlp(self.kanyt._YTDLP_EXEC, 'echo https://muxed/stream\n')
        self.assertEqual(self.kanyt.get_stream_url("abc"), "https://muxed/stream")

    def test_play_resolves_item_and_listing_has_context_menu(self):
        _fake_ytdlp(self.kanyt._YTDLP_EXEC, 'printf "https://v/v.m3u8\\nhttps://a/a.m3u8\\n"\n')
        sys.argv = ["plugin://plugin.video.myyoutubers/", "7", ""]
        sys.modules.pop("default", None)
        sys.path.insert(0, ADDON)
        default = importlib.import_module("default")
        default.kanyt._YTDLP_EXEC = self.kanyt._YTDLP_EXEC
        default.kanyt._MASTER_DIR = self.master
        RESOLVED.clear()
        default.router("?mode=play&id=abc&title=t")
        self.assertTrue(RESOLVED[0][0])
        self.assertTrue(RESOLVED[0][1].path.endswith("abc.m3u8"))
        default.add_video({"id": "abc", "title": "t", "thumb": ""}, 0, 1)
        item = default.xbmcplugin.addDirectoryItem.call_args[0]
        self.assertIn("mode=play", item[1])
        self.assertFalse(item[3])
        labels = [l for l, _ in item[2].menu]
        self.assertEqual(
            labels, ["Play from start", "Download", "Add to playlist", "Remove from playlist"]
        )

    def test_games_scan_and_launch_record_last_played(self):
        roms = os.path.join(self.tmp, "roms")
        os.makedirs(os.path.join(roms, "NES"))
        rom = os.path.join(roms, "NES", "Contra (USA).nes")
        open(rom, "w").close()
        with open(os.path.join(roms, "NES", "Contra (USA).json"), "w") as f:
            f.write('{"released": "1988-02-09"}')
        sys.argv = ["plugin://plugin.video.myyoutubers/", "7", ""]
        sys.modules.pop("default", None)
        sys.path.insert(0, ADDON)
        default = importlib.import_module("default")
        default.ADDON.getSetting.side_effect = lambda k: roms if k == "games_dir" else ""
        games = default.scan_games()
        self.assertEqual([(g["title"], g["system"], g["released"]) for g in games],
                         [("Contra", "nes", "1988-02-09")])
        default.router("?mode=game&id=" + games[0]["id"])
        default.xbmc.executebuiltin.assert_called_with('PlayMedia("{}")'.format(rom))
        self.assertGreater(default.load_games()[0]["last_played"], 0)

    def test_row_directory_lists_mixed_items_newest_first(self):
        replies = {
            "VideoLibrary.GetMovies": {"movies": [
                {"movieid": 1, "title": "Heat", "year": 1995, "file": "/m/heat.mkv"}]},
            "VideoLibrary.GetTVShows": {"tvshows": [
                {"tvshowid": 5, "title": "Chan", "tag": ["YouTube"], "watchedepisodes": 0}]},
            "VideoLibrary.GetEpisodes": {"episodes": [
                {"tvshowid": 5, "firstaired": "2024-06-02", "playcount": 0}]},
        }

        def fake_rpc(req):
            import json
            return json.dumps({"jsonrpc": "2.0", "id": 1, "result": replies[json.loads(req)["method"]]})

        sys.argv = ["plugin://plugin.video.myyoutubers/", "7", ""]
        sys.modules.pop("default", None)
        sys.path.insert(0, ADDON)
        default = importlib.import_module("default")
        default.xbmc.executeJSONRPC.side_effect = fake_rpc
        default.xbmcplugin.addDirectoryItem.reset_mock()
        default.router("?mode=row&id=everything")
        added = [c[0] for c in default.xbmcplugin.addDirectoryItem.call_args_list]
        self.assertEqual([(a[2].label, a[1], a[3]) for a in added],
                         [("Chan", "videodb://tvshows/titles/5/", True), ("Heat", "/m/heat.mkv", False)])
        self.assertEqual(added[1][2].props["IsPlayable"], "true")
        default.xbmcplugin.addDirectoryItem.reset_mock()
        default.router("?mode=rows")
        labels = [c[0][2].label for c in default.xbmcplugin.addDirectoryItem.call_args_list]
        self.assertEqual(labels, ["Everything (2)", "Movies (1)", "YouTube channels (1)"])

    def test_catalog_is_cached_for_the_ttl(self):
        import json
        calls = []

        def fake_rpc(req):
            calls.append(json.loads(req)["method"])
            return json.dumps({"jsonrpc": "2.0", "id": 1, "result": {}})

        sys.argv = ["plugin://plugin.video.myyoutubers/", "7", ""]
        sys.modules.pop("default", None)
        sys.path.insert(0, ADDON)
        default = importlib.import_module("default")
        default.xbmc.executeJSONRPC.side_effect = fake_rpc
        default.load_catalog()
        default.load_catalog()
        self.assertEqual(len(calls), 3)  # movies, shows, episodes: once, not twice
        default.ADDON.getSetting.side_effect = lambda k: "0" if k == "catalog_cache_seconds" else ""
        default.load_catalog()
        self.assertEqual(len(calls), 6)  # ttl 0: always fresh


if __name__ == "__main__":
    unittest.main()
