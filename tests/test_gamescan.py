"""Tests for the games scanner (pure Python)."""
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "addon", "resources", "lib"))
import gamescan  # noqa: E402

DAT = '''clrmamepro (
\tname "Sega - Mega Drive - Genesis"
)

game (
\tname "Sonic The Hedgehog (USA, Europe)"
\trom ( name "Sonic The Hedgehog (USA, Europe).md" size 524288 crc 1 )
\treleaseyear 1991
\treleasemonth 6
)

game (
\tname "Streets of Rage (USA)"
\treleaseyear 1991
)
'''


def touch(path, text=""):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)


class ScanTest(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp()
        self.logs = []
        self.log = lambda lvl, msg: self.logs.append((lvl, msg))

    def p(self, *parts):
        return os.path.join(self.root, *parts)

    def test_system_by_folder_then_extension(self):
        self.assertEqual(gamescan.system_for("Mega Drive", "bin"), "genesis")
        self.assertEqual(gamescan.system_for("misc", "nes"), "nes")
        self.assertIsNone(gamescan.system_for("misc", "xyz"))

    def test_titles_strip_region_tags(self):
        self.assertEqual(gamescan.display_title("Sonic The Hedgehog (USA, Europe) [!].md"), "Sonic The Hedgehog")

    def test_parse_release_precision(self):
        self.assertEqual(gamescan.parse_release("1991"), "1991-01-01")
        self.assertEqual(gamescan.parse_release("1991-6"), "1991-06-01")
        self.assertEqual(gamescan.parse_release("1991-06-23"), "1991-06-23")
        self.assertIsNone(gamescan.parse_release("soon"))
        self.assertIsNone(gamescan.parse_release("1991-13"))

    def test_release_sources_in_priority_order(self):
        d = tempfile.mkdtemp()
        touch(os.path.join(d, "x.dat"), DAT)
        db = gamescan.load_release_db(d)
        touch(self.p("Genesis", "Sonic The Hedgehog (USA, Europe).md"))
        touch(self.p("Genesis", "Streets of Rage (USA).md"))
        touch(self.p("Genesis", "Streets of Rage (USA).json"), json.dumps({"released": "1991-08-02"}))
        touch(self.p("NES", "Mystery.nes"))
        touch(self.p("NES", "Nfo Game.nes"))
        touch(self.p("NES", "Nfo Game.nfo"), "<game><releasedate>1986-02</releasedate></game>")
        games = {g["title"]: g for g in gamescan.scan(self.root, release_db=db, log=self.log)}
        self.assertEqual(games["Sonic The Hedgehog"]["released"], "1991-06-01")
        self.assertEqual(games["Sonic The Hedgehog"]["released_source"], "database")
        self.assertEqual(games["Streets of Rage"]["released"], "1991-08-02")
        self.assertEqual(games["Streets of Rage"]["released_source"], "sidecar")
        self.assertEqual(games["Nfo Game"]["released"], "1986-02-01")
        self.assertIsNone(games["Mystery"]["released"])
        self.assertEqual(games["Mystery"]["released_source"], "none")
        self.assertEqual([l for l, m in self.logs if l == "WARN"], ["WARN"])
        self.assertEqual(len(games), 4)  # sidecars are not games

    def test_ignores_wrong_extension_and_rescan_keeps_last_played(self):
        touch(self.p("NES", "Game.nes"))
        touch(self.p("NES", "notes.txt"))
        touch(self.p("NES", "Game.gb"))  # wrong extension for an NES folder
        first = gamescan.scan(self.root, log=self.log)
        self.assertEqual([g["title"] for g in first], ["Game"])
        first[0]["last_played"] = 12345.0
        second = gamescan.scan(self.root, previous=first, log=self.log)
        self.assertEqual(second[0]["last_played"], 12345.0)
        self.assertEqual(second[0]["id"], first[0]["id"])

    def test_missing_release_db_directory_is_empty(self):
        self.assertEqual(gamescan.load_release_db("/nonexistent"), {})


if __name__ == "__main__":
    unittest.main()
