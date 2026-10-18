# Games (design and verification)

Games are catalogued by the add-on (`gamescan.py`, `games.json`) because Kodi
has no games library with a query API.

**Verified** (Kodi 21.3, e2e `test_games`): a generated NES ROM scanned from
`roms/NES/`, launched with `PlayMedia(<rom>)` from the add-on, runs in
RetroPlayer under Xvfb with software GL (`Player.HasGame` becomes true) using
`game.libretro.nestopia`. The core needs `game.controller.nes` from
`controller-topology-project`.

## Decisions

- System: folder name wins (aliases such as "Mega Drive", "Genesis"), else the
  file extension. A file whose extension does not belong to its folder's
  system is skipped.
- Release date, in order: sidecar (`Game.json` or `Game.nfo`), release
  database (libretro-database `metadat/releaseyear` and `releasemonth`
  clrmamepro `.dat` files, matched on the title without region tags), else
  unknown (null, WARN). Precision is month at best; missing parts become 01.
- Last-played and date added carry over across rescans, keyed by path.

## Open

- Bundling libretro-database as a Nix input and building `resources/releases/`
  from it. Today the directory is simply optional.
- Archives (`.zip`) are not scanned; only plain ROM files.
- Art for games (none yet).
