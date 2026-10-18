"""Games: scan a ROM folder, launch a game through the add-on in RetroPlayer.

Run with `nix run .#e2e -- test_games`.
"""
import json
import os
import shutil
import tempfile
import time
import unittest

import harness

kodi = None
workdir = None
roms = None


def make_nes_rom(path):
    """A valid 16 KiB NROM image whose reset vector jumps to itself forever."""
    prg = bytearray(0x4000)
    prg[0] = 0x4C  # JMP $8000
    prg[1], prg[2] = 0x00, 0x80
    prg[0x3FFC], prg[0x3FFD] = 0x00, 0x80  # reset vector -> $8000
    prg[0x3FFA], prg[0x3FFB] = 0x00, 0x80  # NMI
    prg[0x3FFE], prg[0x3FFF] = 0x00, 0x80  # IRQ
    header = b"NES\x1a" + bytes([1, 1, 0, 0]) + bytes(8)
    with open(path, "wb") as f:
        f.write(header + bytes(prg) + bytes(0x2000))


def setUpModule():
    try:
        _setup()
    except BaseException:
        tearDownModule()  # don't leave Kodi holding the RPC port
        raise


def _setup():
    global kodi, workdir, roms
    workdir = tempfile.mkdtemp(prefix="yok-e2e-games-")
    roms = os.path.join(workdir, "roms")
    os.makedirs(os.path.join(roms, "NES"))
    make_nes_rom(os.path.join(roms, "NES", "Test Game (World).nes"))
    with open(os.path.join(roms, "NES", "Test Game (World).json"), "w") as f:
        json.dump({"released": "1987-05"}, f)
    kodi = harness.Kodi(workdir)
    harness.serve(os.environ["YOK_MASTER"], int(os.environ["YOK_PORT"]))
    kodi.start()
    with open(os.path.join(kodi.profile, "settings.xml"), "w") as f:
        f.write('<settings version="2"><setting id="games_dir">%s</setting></settings>' % roms)
    # A freshly created profile drops plugin runs; a restart settles it.
    kodi.restart_kodi()
    kodi.rpc("Addons.ExecuteAddon", addonid=harness.ADDON_ID, params={"mode": "scan_games"})
    kodi._wait(lambda: os.path.exists(os.path.join(kodi.profile, "games.json")), 30, "games.json")
    kodi.rpc("Addons.SetAddonEnabled", addonid="game.libretro.nestopia", enabled=True)


def tearDownModule():
    if kodi:
        kodi.stop()
    harness.stop_servers()
    if workdir and not os.environ.get("YOK_KEEP"):
        shutil.rmtree(workdir, ignore_errors=True)


def games():
    with open(os.path.join(kodi.profile, "games.json")) as f:
        return json.load(f)


class GamesTest(unittest.TestCase):
    def fail_with_log(self, exc):
        tail = "\n".join(kodi.log().splitlines()[-60:])
        raise AssertionError("%s\n--- kodi.log tail ---\n%s" % (exc, tail))

    def test_scan_catalogs_game_with_system_and_release(self):
        (g,) = games()
        self.assertEqual((g["title"], g["system"], g["released"], g["released_source"]),
                         ("Test Game", "nes", "1987-05-01", "sidecar"))

    def test_launch_runs_game_in_retroplayer(self):
        (g,) = games()
        kodi.rpc("Addons.ExecuteAddon", addonid=harness.ADDON_ID, params={"mode": "game", "id": g["id"]})
        try:
            kodi._wait(lambda: kodi.rpc("XBMC.GetInfoBooleans", booleans=["Player.HasGame"])["Player.HasGame"],
                       40, "RetroPlayer to start the game")
        except Exception as e:
            self.fail_with_log(e)
        self.assertGreater(games()[0]["last_played"], 0)


if __name__ == "__main__":
    unittest.main()
