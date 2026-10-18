"""Background service: periodically refresh the channel export and games scan."""
import os
import sys
import time

import xbmc
import xbmcaddon
import xbmcvfs

INITIAL_DELAY = 20  # seconds after login, so startup isn't competing with us
TICK = 30  # seconds between checks; settings changes take effect within one tick

_addon = xbmcaddon.Addon()
ADDON_ID = _addon.getAddonInfo("id")
ADDON_PATH = xbmcvfs.translatePath(_addon.getAddonInfo("path"))
PROFILE = xbmcvfs.translatePath(_addon.getAddonInfo("profile"))

sys.path.insert(0, os.path.join(ADDON_PATH, "resources", "lib"))

import refresh  # noqa: E402

LEVELS = {
    "DEBUG": xbmc.LOGDEBUG,
    "INFO": xbmc.LOGINFO,
    "WARN": xbmc.LOGWARNING,
    "ERROR": xbmc.LOGERROR,
}


def log(level, message):
    xbmc.log("[{}] {}".format(ADDON_ID, message), LEVELS.get(level, xbmc.LOGINFO))


def refresh_once(addon):
    """One pass over the channel export and the games folder."""
    get = addon.getSetting
    refresh.configure(get)
    root = xbmcvfs.translatePath(refresh.library_dir(PROFILE, get))
    max_videos = refresh.setting_int(get, "library_max_videos", 50)
    os.makedirs(PROFILE, exist_ok=True)
    if refresh.export_channels(PROFILE, root, max_videos, log):
        xbmc.executebuiltin('UpdateLibrary(video,"{}")'.format(root))
    if get("games_dir"):
        refresh.scan_games(
            PROFILE, os.path.join(ADDON_PATH, "resources", "releases"), get("games_dir"), log
        )


class Monitor(xbmc.Monitor):
    """Drops the home-rows catalog cache whenever the video library changes."""

    def onNotification(self, sender, method, data):
        if method.startswith("VideoLibrary.On") or method == "Player.OnStop":
            if refresh.invalidate_catalog_cache(PROFILE):
                log("DEBUG", "catalog cache dropped on {}".format(method))


def main():
    monitor = Monitor()
    log("INFO", "refresh service started")
    if monitor.waitForAbort(INITIAL_DELAY):
        return
    last_run = None
    while True:
        addon = xbmcaddon.Addon()  # fresh instance: picks up changed settings
        minutes = refresh.setting_int(addon.getSetting, "media_refresh_minutes", 0)
        due = last_run is None or time.monotonic() - last_run >= minutes * 60
        if minutes > 0 and due:
            last_run = time.monotonic()
            try:
                refresh_once(addon)
            except Exception as e:
                log("ERROR", "refresh failed: {}".format(e))
        if monitor.waitForAbort(TICK):
            break
    log("INFO", "refresh service stopped")


if __name__ == "__main__":
    main()
