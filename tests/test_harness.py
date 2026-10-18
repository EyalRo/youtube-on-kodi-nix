"""The e2e harness's startup retry (no Kodi needed)."""
import importlib.util
import os
import tempfile
import unittest

# Loaded by path: putting tests/e2e on sys.path would make its test_* modules
# shadow the unit tests of the same name.
_spec = importlib.util.spec_from_file_location(
    "e2e_harness", os.path.join(os.path.dirname(__file__), "e2e", "harness.py"))
harness = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(harness)


class DeadProc:
    def wait(self, *a):
        return 139


class StartupRetryTest(unittest.TestCase):
    def kodi(self, retries):
        k = harness.Kodi(tempfile.mkdtemp(), startup_retries=retries)
        k.launches = 0

        def once():
            k.launches += 1
            k.kodi_proc = DeadProc()
            k.procs.append(k.kodi_proc)
            if k.launches <= k.fail_times:
                raise harness.KodiDied("Kodi exited with status 139 during startup")

        k._launch_kodi_once = once
        return k

    def test_a_crashed_startup_is_retried(self):
        k = self.kodi(2)
        k.fail_times = 2
        k._launch_kodi()
        self.assertEqual(k.launches, 3)
        self.assertEqual(len(k.procs), 1, "dead processes are dropped")

    def test_gives_up_after_the_retries(self):
        k = self.kodi(1)
        k.fail_times = 5
        with self.assertRaises(harness.KodiDied):
            k._launch_kodi()
        self.assertEqual(k.launches, 2)

    def test_no_retries_means_crashes_surface(self):
        k = self.kodi(0)
        k.fail_times = 1
        with self.assertRaises(harness.KodiDied):
            k._launch_kodi()
        self.assertEqual(k.launches, 1)


if __name__ == "__main__":
    unittest.main()
