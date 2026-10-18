"""Channel export and games scan, shared by the plugin and the background service.

No Kodi imports: callers pass the profile path, the `getSetting` callable and a
`log(level, message)` callable.
"""
import json
import os

import gamescan
import kanyt
import libexport


def setting_int(get_setting, name, default):
    try:
        return int(get_setting(name) or default)
    except (ValueError, TypeError):
        return default


def configure(get_setting):
    """Apply the add-on's settings to kanyt's module-level tunables."""
    kanyt.REFRESH_INTERVAL = setting_int(get_setting, "refresh_hours", 24) * 3600
    kanyt.CHANNEL_EXPIRE = setting_int(get_setting, "channel_expire_days", 30) * 86400
    kanyt.RETRY_COOLDOWN = setting_int(get_setting, "retry_cooldown_minutes", 10) * 60
    kanyt.SEARCH_MAX_AGE = setting_int(get_setting, "search_max_age_days", 7) * 86400
    kanyt.MAX_HEIGHT = setting_int(get_setting, "max_height", kanyt.MAX_HEIGHT)


def catalog_cache_path(profile):
    return os.path.join(profile, "catalog_cache.json")


def invalidate_catalog_cache(profile):
    """Drop the cached home-rows catalog (the library or a watched state changed)."""
    try:
        os.remove(catalog_cache_path(profile))
        return True
    except OSError:
        return False


def library_dir(profile, get_setting):
    """Folder the channel export writes to: the setting, else <profile>/library."""
    return get_setting("library_dir") or os.path.join(profile, "library")


def export_channels(profile, root, max_videos, log):
    """Write every configured channel as a library TV show under `root`.

    Returns the number of files created or changed.
    """
    conf = os.path.join(profile, "channels.conf")
    cache = os.path.join(profile, "cache")
    changed = exported = 0
    for c in kanyt.read_channels_conf(conf):
        ch = kanyt.resolve_cached(c["query"], cache)
        if not ch:
            log("ERROR", "cannot resolve channel {!r}; skipped".format(c["query"]))
            continue
        ch = {"id": ch["id"], "name": c.get("name") or ch["name"]}
        try:
            videos = kanyt.get_dated_videos(ch["id"], cache, end=max_videos)
            changed += libexport.export_channel(root, ch, videos, max_videos, log)
            exported += 1
        except Exception as e:
            log("ERROR", "export of {} failed: {}".format(ch["name"], e))
    log("INFO", "library export done: {} channel(s) under {}, {} file(s) changed".format(
        exported, root, changed))
    return changed


def load_games(profile):
    try:
        with open(os.path.join(profile, "games.json")) as f:
            data = json.load(f)
        if isinstance(data, list):
            return data
    except (OSError, ValueError):
        pass
    return []


def scan_games(profile, releases_dir, games_dir, log):
    """Rescan `games_dir` into games.json under `profile`; returns the catalog."""
    if not games_dir or not os.path.isdir(games_dir):
        log("ERROR", "games folder {!r} is not set or does not exist".format(games_dir))
        return []
    games = gamescan.scan(
        games_dir, load_games(profile), gamescan.load_release_db(releases_dir), log
    )
    kanyt.atomic_write(os.path.join(profile, "games.json"), games)
    return games
