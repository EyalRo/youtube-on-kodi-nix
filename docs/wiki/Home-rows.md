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
