"""Scan ROM folders into a games catalog (games.json).

Pure Python, no Kodi imports: callers pass a `log(level, message)` callable.

ROMs live in one folder per system under the root (`roms/NES/Game.nes`).
The folder name decides the system when it is a known one; otherwise the file
extension does. A game's release date comes from, in order:

1. a sidecar next to the ROM: `Game.json` {"released": "1991-06-23"} or
   `Game.nfo` with <releasedate>;
2. a release database: clrmamepro-style .dat files (the libretro-database
   `metadat/releaseyear` and `releasemonth` format), matched on the title with
   region and revision tags removed;
3. unknown: the game is catalogued with `released` null and a WARN is logged.
"""
import datetime
import hashlib
import json
import os
import re
import xml.etree.ElementTree as ET

# system id -> (display name, file extensions, folder-name aliases)
SYSTEMS = {
    "nes": ("NES", {"nes", "unf", "unif"}, {"nes", "famicom", "nintendo", "nintendoentertainmentsystem"}),
    "snes": ("SNES", {"sfc", "smc"}, {"snes", "supernintendo", "superfamicom", "supernes"}),
    "genesis": ("Genesis", {"md", "gen", "smd"}, {"genesis", "megadrive", "segagenesis", "segamegadrive"}),
    "sms": ("Master System", {"sms"}, {"sms", "mastersystem", "segamastersystem"}),
    "gamegear": ("Game Gear", {"gg"}, {"gg", "gamegear", "segagamegear"}),
    "gb": ("Game Boy", {"gb"}, {"gb", "gameboy"}),
    "gbc": ("Game Boy Color", {"gbc"}, {"gbc", "gameboycolor"}),
    "gba": ("Game Boy Advance", {"gba"}, {"gba", "gameboyadvance"}),
    "n64": ("Nintendo 64", {"z64", "n64", "v64"}, {"n64", "nintendo64"}),
}

_TAGS = re.compile(r"\s*[\(\[][^\)\]]*[\)\]]")


def _squash(text):
    return re.sub(r"[^a-z0-9]", "", text.lower())


def system_for(folder, ext):
    """System id for a ROM in `folder` with extension `ext`, or None."""
    key = _squash(folder)
    for sid, (_, _, aliases) in SYSTEMS.items():
        if key in aliases:
            return sid
    for sid, (_, exts, _) in SYSTEMS.items():
        if ext in exts:
            return sid
    return None


def display_title(filename):
    return _TAGS.sub("", os.path.splitext(filename)[0]).strip() or filename


def title_key(name):
    return _squash(_TAGS.sub("", name))


def parse_release(value):
    """'1991', '1991-06' or '1991-06-23' -> ISO date (missing parts are 01)."""
    m = re.fullmatch(r"\s*(\d{4})(?:-(\d{1,2}))?(?:-(\d{1,2}))?\s*", str(value or ""))
    if not m:
        return None
    try:
        return datetime.date(int(m.group(1)), int(m.group(2) or 1), int(m.group(3) or 1)).isoformat()
    except ValueError:
        return None


def read_sidecar(rom_path):
    base = os.path.splitext(rom_path)[0]
    try:
        with open(base + ".json", encoding="utf-8") as f:
            return parse_release(json.load(f).get("released"))
    except (OSError, ValueError, AttributeError):
        pass
    try:
        return parse_release(ET.parse(base + ".nfo").getroot().findtext("releasedate"))
    except (OSError, ET.ParseError):
        return None


def load_release_db(directory):
    """Read every .dat in `directory` into {title_key: {"year", "month"}}.

    Files named *year* contribute `releaseyear`, *month* contribute
    `releasemonth`, as in libretro-database. Missing directory -> empty.
    """
    db = {}
    if not directory or not os.path.isdir(directory):
        return db
    for fn in sorted(os.listdir(directory)):
        if not fn.endswith(".dat"):
            continue
        with open(os.path.join(directory, fn), encoding="utf-8", errors="replace") as f:
            text = f.read()
        for block in re.finditer(r'game\s*\(\s*name\s+"([^"]+)"(.*?)\n\)', text, re.S):
            name, body = block.group(1), block.group(2)
            for field in ("releaseyear", "releasemonth"):
                m = re.search(field + r"\s+(\d+)", body)
                if m:
                    db.setdefault(title_key(name), {})[field[7:]] = int(m.group(1))
    return db


def lookup_release(db, filename):
    entry = db.get(title_key(os.path.splitext(filename)[0]))
    if not entry or "year" not in entry:
        return None
    return parse_release("{}-{}".format(entry["year"], entry.get("month", 1)))


def scan(root, previous=None, release_db=None, log=lambda level, msg: None):
    """Return the games catalog for `root`: a list of dicts, sorted by title.

    `previous` is the earlier catalog; last_played carries over by path.
    """
    prior = {g["path"]: g for g in (previous or [])}
    release_db = release_db or {}
    games = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        rel_dir = os.path.relpath(dirpath, root)
        if rel_dir == ".":
            continue
        folder = rel_dir.split(os.sep)[0]
        for fn in sorted(filenames):
            ext = os.path.splitext(fn)[1].lstrip(".").lower()
            sid = system_for(folder, ext)
            if sid is None or ext in ("json", "nfo", "txt", "png", "jpg", "srm", "sav"):
                continue
            if ext not in SYSTEMS[sid][1]:
                log("DEBUG", "skipping {}: {!r} is not a {} extension".format(fn, ext, SYSTEMS[sid][0]))
                continue
            path = os.path.join(dirpath, fn)
            released, source = read_sidecar(path), "sidecar"
            if not released:
                released, source = lookup_release(release_db, fn), "database"
            if not released:
                released, source = None, "none"
                log("WARN", "no release date for {} ({}); add {}.json".format(
                    fn, SYSTEMS[sid][0], os.path.splitext(fn)[0]))
            old = prior.get(path, {})
            games.append({
                "id": hashlib.sha1(os.path.relpath(path, root).encode()).hexdigest()[:12],
                "title": display_title(fn),
                "system": sid,
                "path": path,
                "released": released,
                "released_source": source,
                "added": old.get("added") or os.path.getmtime(path),
                "last_played": old.get("last_played"),
            })
    games.sort(key=lambda g: (g["system"], g["title"].lower()))
    log("INFO", "scanned {}: {} game(s) across {} system(s)".format(
        root, len(games), len({g["system"] for g in games})))
    return games
