"""Home-screen rows: filters and sorts over one mixed media catalog.

Pure Python, no Kodi imports. Callers pass data already fetched from the Kodi
video library (movies, TV shows, episodes as JSON-RPC returns them) and from
games.json; this module normalizes it to items and builds rows.

Every item has one sort date, newest first, undated last:
  movie            release date (`premiered`, else `year`)
  TV show/channel  air date of its newest episode
  game             release date
A show tagged YouTube is a `channel`; other shows are `show`.
"""
import gamescan

YOUTUBE_TAG = "YouTube"
LIBRARY_SHOW_URL = "videodb://tvshows/titles/{}/"


def _art(raw):
    raw = raw or {}
    return {
        "thumb": raw.get("poster") or raw.get("thumb") or "",
        "poster": raw.get("poster") or raw.get("thumb") or "",
        "fanart": raw.get("fanart") or "",
    }


def movie_items(movies):
    out = []
    for m in movies:
        date = gamescan.parse_release(m.get("premiered") or m.get("year") or "")
        out.append({
            "kind": "movie", "key": "movie:%s" % m["movieid"], "title": m["title"],
            "date": date, "art": _art(m.get("art")), "url": m["file"],
            "is_folder": False, "playable": True, "label2": "Movie",
        })
    return out


def show_items(shows, episodes):
    """One item per show; `date` is its newest aired episode."""
    newest, unwatched = {}, {}
    for e in episodes:
        date, sid = e.get("firstaired") or None, e["tvshowid"]
        if not date:
            continue
        if date > newest.get(sid, ""):
            newest[sid] = date
        if not e.get("playcount") and date > unwatched.get(sid, ""):
            unwatched[sid] = date
    out = []
    for s in shows:
        sid = s["tvshowid"]
        channel = YOUTUBE_TAG in (s.get("tag") or [])
        out.append({
            "kind": "channel" if channel else "show", "key": "show:%s" % sid, "title": s["title"],
            "date": newest.get(sid), "art": _art(s.get("art")), "url": LIBRARY_SHOW_URL.format(sid),
            "is_folder": True, "playable": False, "label2": "YouTube" if channel else "TV show",
            "watched": s.get("watchedepisodes") or 0, "unwatched_date": unwatched.get(sid),
        })
    return out


def game_items(games, play_url):
    """`play_url(game)` builds the URL that launches a game."""
    out = []
    for g in games:
        name = gamescan.SYSTEMS.get(g["system"], (g["system"],))[0]
        out.append({
            "kind": "game", "key": "game:%s" % g["id"], "title": g["title"], "date": g.get("released"),
            "art": _art(None), "url": play_url(g), "is_folder": False, "playable": False,
            "label2": name, "system": g["system"],
        })
    return out


def sort_newest(items, field="date"):
    """Newest first by `field`; undated last; ties and undated by title."""
    ordered = sorted(items, key=lambda i: i["title"].lower())
    dated = [i for i in ordered if i.get(field)]
    undated = [i for i in ordered if not i.get(field)]
    dated.sort(key=lambda i: i[field], reverse=True)  # stable: equal dates stay by title
    return dated + undated


def _kinds(*kinds):
    return lambda i: i["kind"] in kinds


FIXED_ROWS = [
    # id, label, filter, sort field
    ("everything", "Everything", lambda i: True, "date"),
    ("movies", "Movies", _kinds("movie"), "date"),
    ("tvshows", "TV shows", _kinds("show"), "date"),
    ("channels", "YouTube channels", _kinds("channel"), "date"),
    ("unwatched", "Shows with unwatched episodes",
     lambda i: i["kind"] in ("show", "channel") and i["watched"] > 0 and i["unwatched_date"],
     "unwatched_date"),
    ("games", "Games", _kinds("game"), "date"),
]


def row_definitions(items):
    """Fixed rows plus one games row per system present, as (id, label, filter, field)."""
    rows = list(FIXED_ROWS)
    systems = sorted({i["system"] for i in items if i["kind"] == "game"},
                     key=lambda s: gamescan.SYSTEMS.get(s, (s,))[0])
    for sid in systems:
        label = gamescan.SYSTEMS.get(sid, (sid,))[0]
        rows.append(("games:" + sid, label, (lambda s: lambda i: i["kind"] == "game" and i["system"] == s)(sid), "date"))
    return rows


def list_rows(items):
    """[(id, label, count)] for every row with at least one item (Everything always)."""
    out = []
    for rid, label, flt, _ in row_definitions(items):
        n = sum(1 for i in items if flt(i))
        if n or rid == "everything":
            out.append((rid, label, n))
    return out


def build_row(row_id, items):
    """The items of one row in display order; unknown ids give None."""
    for rid, _, flt, field in row_definitions(items):
        if rid == row_id:
            return sort_newest([i for i in items if flt(i)], field)
    return None
