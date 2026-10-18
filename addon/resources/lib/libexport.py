"""Export YouTube channels as a Kodi TV-show library (.strm + NFO files).

Pure Python, no Kodi imports: callers pass a `log(level, message)` callable.

Layout, one folder per channel under `root`:

    <Channel> [<channel id>]/tvshow.nfo
    <Channel> [<channel id>]/S<year>E<n> - <yyyy-mm-dd> - <title> [<video id>].strm
    <Channel> [<channel id>]/S<year>E<n> - <yyyy-mm-dd> - <title> [<video id>].nfo

Season is the upload year. The episode number is day-of-year * 100 plus the
rank among that day's uploads, so numbers stay stable when older videos are
backfilled later. Files are never deleted: a video that falls out of the
latest-N window keeps its library entry and watched state.
"""
import datetime
import os
import re
from xml.sax.saxutils import escape

PLUGIN_URL = "plugin://plugin.video.myyoutubers/"
TAG = "YouTube"

_UNSAFE = re.compile(r'[<>:"/\\|?*\x00-\x1f]')


def safe_name(text, limit=80):
    text = _UNSAFE.sub(" ", text or "")
    text = re.sub(r"\s+", " ", text).strip(" .")
    return text[:limit].rstrip(" .") or "untitled"


def parse_date(video):
    """Return the upload date of a video dict as a date, or None."""
    published = video.get("published")
    if published:
        try:
            return datetime.datetime.fromisoformat(published.replace("Z", "+00:00")).date()
        except ValueError:
            pass
    upload_date = video.get("upload_date")
    if upload_date and re.fullmatch(r"\d{8}", str(upload_date)):
        try:
            return datetime.datetime.strptime(upload_date, "%Y%m%d").date()
        except ValueError:
            pass
    return None


def _write_if_changed(path, text):
    """Write atomically; leave the file (and its mtime) alone if unchanged."""
    try:
        with open(path, encoding="utf-8") as f:
            if f.read() == text:
                return False
    except OSError:
        pass
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(tmp, path)
    return True


def _xml(tag, text):
    return "  <{0}>{1}</{0}>\n".format(tag, escape(str(text)))


def show_nfo(channel):
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<tvshow>\n'
        + _xml("title", channel["name"])
        + _xml("plot", "YouTube channel " + channel["name"])
        + _xml("tag", TAG)
        + '  <uniqueid type="youtube" default="true">{}</uniqueid>\n'.format(escape(channel["id"]))
        + "</tvshow>\n"
    )


def episode_nfo(channel, video, date, episode):
    out = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\n<episodedetails>\n'
        + _xml("title", video["title"])
        + _xml("showtitle", channel["name"])
        + _xml("season", date.year)
        + _xml("episode", episode)
        + _xml("aired", date.isoformat())
        + _xml("plot", video["title"])
    )
    if video.get("thumb"):
        out += _xml("thumb", video["thumb"])
    out += '  <uniqueid type="youtube" default="true">{}</uniqueid>\n'.format(escape(video["id"]))
    return out + "</episodedetails>\n"


def strm_url(video):
    return "{}?mode=play&id={}&title={}".format(
        PLUGIN_URL, video["id"], re.sub(r"[&=#?%]", " ", video["title"]).strip()
    )


def export_channel(root, channel, videos, max_videos=50, log=lambda level, msg: None):
    """Write the show and episode files for one channel.

    `channel` is {"id", "name"}; `videos` are dicts with id, title, thumb and a
    date (`published` ISO string or `upload_date` YYYYMMDD). Returns the number
    of files created or changed.
    """
    dated, skipped = [], 0
    for v in videos:
        date = parse_date(v)
        if date:
            dated.append((date, v))
        else:
            skipped += 1
    if skipped:
        log("WARN", "{}: {} video(s) without an upload date not exported".format(channel["name"], skipped))
    dated.sort(key=lambda dv: (dv[0], dv[1]["id"]), reverse=True)
    dated = dated[:max_videos]

    folder = os.path.join(root, "{} [{}]".format(safe_name(channel["name"]), channel["id"]))
    os.makedirs(folder, exist_ok=True)
    changed = int(_write_if_changed(os.path.join(folder, "tvshow.nfo"), show_nfo(channel)))

    # Rank within a day over the whole window, oldest first, for stable numbers.
    per_day = {}
    for date, v in sorted(dated, key=lambda dv: (dv[0], dv[1]["id"])):
        rank = per_day.get(date, 0)
        per_day[date] = rank + 1
        episode = date.timetuple().tm_yday * 100 + rank
        # Kodi numbers episodes from the filename first; an explicit SxxExx
        # keeps it from matching patterns inside the title or video id.
        base = os.path.join(folder, "S{}E{} - {} - {} [{}]".format(
            date.year, episode, date.isoformat(), safe_name(v["title"]), v["id"]))
        changed += _write_if_changed(base + ".strm", strm_url(v) + "\n")
        changed += _write_if_changed(base + ".nfo", episode_nfo(channel, v, date, episode))
    log("INFO", "{}: exported {} video(s), {} file(s) changed".format(channel["name"], len(dated), changed))
    return changed
