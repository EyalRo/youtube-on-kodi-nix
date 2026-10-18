"""Experiment: does Kodi survive the home screen's widget scripts starting at once?

Kodi 21.3 on its default Python (3.13) segfaulted in libpython at startup with 15
home widgets. This restarts Kodi with the 15-widget skin YOK_STRESS_ROUNDS times
(default 8) and counts crashes. Run via the experiment apps only:

  nix run .#e2e-stress          # default Python
  nix run .#e2e-stress-py312    # Kodi built against Python 3.12
"""
import os
import re
import shutil
import tempfile
import time
import unittest

import harness

ROUNDS = int(os.environ.get("YOK_STRESS_ROUNDS", "8"))
SETTLE = 12  # seconds to let the home widgets run before checking Kodi

kodi = None
workdir = None


def setUpModule():
    if not os.environ.get("YOK_STRESS"):  # only meaningful with the 15-widget skin
        raise unittest.SkipTest("run via nix run .#e2e-stress or .#e2e-stress-py312")
    try:
        _setup()
    except BaseException:
        tearDownModule()
        raise


def _setup():
    global kodi, workdir
    workdir = tempfile.mkdtemp(prefix="yok-e2e-stress-")
    kodi = harness.Kodi(workdir, skin=True)
    kodi.start()
    kodi.restart_kodi()  # the skin only becomes active on the next launch


def tearDownModule():
    if kodi:
        kodi.stop()
    harness.stop_servers()
    if workdir and not os.environ.get("YOK_KEEP"):
        shutil.rmtree(workdir, ignore_errors=True)


def widget_scripts_started():
    """How many row/system plugin scripts the current Kodi run started."""
    return len(re.findall(r"running add-on script Youtube On Kodi\(.*\?mode=(?:row|systems)", kodi.log()))


class StressTest(unittest.TestCase):
    def one_startup(self):
        """Start Kodi once; returns "ok" or how it crashed."""
        try:
            if kodi.alive():
                kodi.restart_kodi()
            else:
                kodi.relaunch_after_crash()
        except harness.KodiHung:
            return "hung on quit"
        except harness.KodiDied:
            return "crashed during startup"
        time.sleep(SETTLE)
        return "ok" if kodi.alive() else "crashed after startup"

    def test_home_with_15_widgets_survives_repeated_startups(self):
        outcomes, started = [], []
        for _ in range(ROUNDS):
            outcome = self.one_startup()
            outcomes.append(outcome)
            if outcome == "ok":
                started.append(widget_scripts_started())
        crashes = [o for o in outcomes if o != "ok"]
        print("\nSTRESS RESULT: %d crash(es) in %d startups %r; widget scripts started per good run: %r"
              % (len(crashes), ROUNDS, crashes, started))
        self.assertTrue(started and min(started) >= 10, "the home screen did not start its widget scripts")
        self.assertEqual(crashes, [], "Kodi crashed in %d of %d startups" % (len(crashes), ROUNDS))


if __name__ == "__main__":
    unittest.main()
