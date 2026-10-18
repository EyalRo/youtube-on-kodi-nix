# Home rows (design and verification)

`rows.py` (pure) turns the Kodi video library (movies, TV shows, episodes via
JSON-RPC) and `games.json` into items and rows; `default.py` exposes them as
plugin directories `mode=rows` (the row list) and `mode=row&id=<row>`.

**Verified** (Kodi 21.3, e2e `test_rows`): with a YouTube channel, a plain TV
show, a movie and a game, `Everything` lists them in release/air-date order,
the per-type rows filter correctly, and `unwatched` shows the channel after one
of its videos is marked watched.

## Rules

- One sort date per item, newest first, undated last, ties by title: movie =
  `premiered` (else `year`), show/channel = newest aired episode, game =
  release date.
- Channel = TV show tagged `YouTube`.
- "Shows with unwatched episodes": at least one watched episode and at least
  one dated unwatched episode; sorted by the newest unwatched episode.
- Rows with no items are hidden, except Everything. One games row per system
  present in `games.json`.
- Shows open their Kodi library page (`videodb://tvshows/titles/<id>/`),
  movies play, games launch through the add-on.

## Notes and limits

- The catalog is built on every directory request: three library queries plus
  `games.json`. `GetEpisodes` has no limit, which is fine for personal libraries
  and is the thing to page or cache first if it gets slow.
- Kodi runs one plugin at a time: a second `Addons.ExecuteAddon` pre-empts the
  first, and a call straight after a fresh profile's first start is dropped.
  The e2e tests work around both.

## The skin (Estuary Rows)

`skin.nix` + `skin/estuary-rows.patch` + `skin/Includes_Rows.xml`. Estuary's
home screen has hardcoded widget rows and no hook for custom ones, so the
rows needed a skin. The skin is Estuary from the Kodi package with a patch:
a first menu entry (`yokrows`) and a group of `WidgetListPoster` widgets whose
`content_path` is the add-on's row directories. The patch touches only
`Home.xml`, `Includes.xml` and `addon.xml`.

**Verified** (e2e `SkinTest`): the skin loads, "Rows" is the first menu entry
and the default selection, and widgets 8100..8600 hold the right items in the
right order. `YOK_SCREENSHOTS=<dir> nix run .#e2e -- test_rows` saves
`home-rows.png`.

![Home rows](images/home-rows.png)

Findings worth remembering:

- Only enabled add-ons can be the skin, and a skin that is not one of Kodi's
  system add-ons starts out disabled. Kodi loads the skin before JSON-RPC is
  up and rewrites `guisettings.xml` on exit, so the e2e harness enables the
  skin, quits Kodi, sets `lookandfeel.skin` while it is stopped, and relaunches.
- A skin must be built with `buildKodiAddon` for `kodi.withPackages` to include it.
- The patch is applied at build time; a Kodi update that changes those Estuary
  files makes the build fail rather than silently dropping the rows.

Not done: artwork placeholders (cards show Kodi's generic icon without art),
per-system game rows in the widgets, launching a game from a widget, and
catalog caching.
