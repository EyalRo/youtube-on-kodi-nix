# Unified "next up" view for shows (design, future)

Goal: one view over TV shows and YouTube channels, ordered by the show whose
last unwatched episode aired most recently.

## Definitions (decide which)

1. **Newest unwatched**: for each show, the most recently *aired* episode
   with `playcount == 0`; shows sorted by that date, descending. Matches
   "what's new".
2. **Next up**: the earliest unwatched episode after the last watched one;
   shows sorted by the air date of the newest unwatched episode.

## Approach

- Built-ins cover parts: "In progress TV shows", "Recently added episodes",
  smart playlists with `playcount is 0` sorted by `aired`. None groups by
  show while sorting by per-show newest unwatched air date.
- With the YouTube library from [Channels as TV shows](Channels-as-TV-shows)
  option B, the data is all in the library, so a **plugin directory** can
  compute the view with JSON-RPC and present it to any skin:
  `VideoLibrary.GetEpisodes` with filter `playcount is 0`, properties
  `tvshowid, season, episode, firstaired, art, showtitle`, sort `firstaired`
  descending; keep the first episode per `tvshowid` (definition 1), or sort
  ascending and take the first per show for "next up" (definition 2).
  Return as episode items grouped by show, with the show poster.
- Without the library (option A only), the add-on merges its own channel
  videos with `VideoLibrary` results in the same plugin directory.
- A skin can also call this directory from a home-screen widget; see the
  skin research (not started).

## Open questions

- Performance of `GetEpisodes` on large libraries (limit + paging).
- How to treat partially watched (resume) episodes: count as unwatched but
  show progress.
- Cache for YouTube channels not in the library.
