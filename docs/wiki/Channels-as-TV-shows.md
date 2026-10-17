# Channels as TV shows (design, future)

Goal: treat a YouTube channel like a TV show in a separate category, with
seasons/episodes, watched state and library views.

## Options

**A. Plugin directories with episode metadata (no library).** The add-on
already exposes channel listings. Set `xbmcplugin.setContent(handle,
"episodes")` and per item `setInfo("video", {mediatype: "episode",
tvshowtitle, season, episode, aired, plot, duration, playcount})`, resume via
`ResumeTime`/`TotalTime`. Skins render these like TV episodes. Cheap, no sync,
but not in the Kodi library: no library watched flag, no library widgets.

**B. Generated library (`.strm` + NFO).** A background add-on service
writes, per channel, a folder under e.g. `/var/lib/kodi/youtube/<Channel>/`:
`tvshow.nfo` (title, plot, art, `uniqueid type="youtube"`), and per video
`<date> - <title>.strm` containing the `plugin://...?mode=play&id=...` URL
plus `<same>.nfo` with `<season>` (upload year), `<episode>` (order in year),
`<aired>`, `<runtime>`, `<thumb>`. Add the folder as a Video source with
content "TV shows" and the "Local information only" scraper; trigger
`VideoLibrary.Scan` with the directory after each refresh. Kodi's wiki states
that NFO files are always read first, and a TV show needs a `tvshow.nfo`.
Gets real library behaviour: watched flags, resume, in-progress, widgets.

Risk (Hypothesis): a forum thread reports the library storing the video ID
instead of the full path for `plugin://` strm files; verify ours resolves
correctly. Library designed for finite shows; thousands of "episodes" per
channel need a limit (latest N plus on demand).

**C. Video node.** Add a custom video node (`special://profile/library/video/
youtube.xml`) or tag/genre "YouTube" so the category appears beside Movies/TV
shows and is filterable. Combines with B.

## Recommendation

Do A first (small, immediately improves every skin), then B + C behind a
setting. Season = upload year avoids the arbitrary "season 1 with 2000
episodes". Keep per-video watched state in one place: library when B is on,
add-on history otherwise.

## Open questions

- Keep playlists as seasons? (Playlist = season is natural, but videos can be
  in several.)
- Shorts and live streams: filter or separate "season".
- Art: channel avatar/banner for poster/fanart (yt-dlp provides thumbnails).
