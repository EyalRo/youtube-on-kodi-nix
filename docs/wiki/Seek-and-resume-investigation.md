# Seek and resume investigation

**Report:** skipping ahead in a video, or starting at 20 minutes in, failed.

Two independent bugs, both reproduced and fixed. Tracked in the issue
"Seeking and resuming fail".

## Bug 1: resume ignored (our regression) - Verified

The first fork commit replaced `Player.play()` with `setResolvedUrl()` and set
the `StartOffset` list-item property to carry the resume position. Kodi
**ignores `StartOffset` on items resolved by a plugin**: playback started at
0:00. Worse, the history tracker then saved the position near 0 over the
stored 20:00, destroying the resume point.

Fix: after `setResolvedUrl`, wait until the player is playing and `seekTime()`
to the saved position (`_seek_when_playing` in `addon/default.py`), before the
tracker starts saving. Covered by `test_resume_from_history`.

## Bug 2: seeking breaks fragmented-MP4 HLS in Kodi 21 - Verified

The add-on plays YouTube as an HLS master playlist pairing a video-only and an
audio-only media playlist. Observations with generated fixtures (H.264 + AAC,
25 min, 5 s segments):

| Segments | Kodi 21.3 ffmpeg demuxer | `inputstream.adaptive` 21.5 |
|----------|--------------------------|-----------------------------|
| MPEG-TS  | seeks correctly          | seeks correctly             |
| fMP4 (`#EXT-X-MAP`) | **seek breaks the video** | seeks correctly |

With fMP4, after any seek Kodi's position sticks at about `-0.1 s`. The log
shows the demuxer opening segments 239, 240, ... 299 one after another and
never producing a frame. Plain `ffmpeg 6.1` reproduces it outside Kodi:

```
ffmpeg -ss 100 -i http://.../video.m3u8 -t 3 -f null -
[NULL] Invalid NAL unit size (-1145703161 > 3312).
[NULL] missing picture in access unit with size 3316
```

Narrowing:

- Fails on the **video-only** fMP4 playlist by itself, so it is not the
  separate-audio-rendition master structure. The audio-only fMP4 playlist
  seeks fine.
- `ffmpeg 9.0.1` plays the same fixture correctly after a seek. Kodi 21
  bundles ffmpeg 6.1.x (nixpkgs: 6.1.3), so the add-on cannot get the fix.
- The pre-seek decode is fine; the symptom matches the init section
  (`#EXT-X-MAP` avcC) not being applied to segments read after a seek.

A first manual check wrongly looked fine: Kodi's *clock* reached the target
time, but the picture was broken. The e2e check therefore requires the
player to be at the target **and advancing** after the seek, which fails for
the ffmpeg path.

### Fix

Play through `inputstream.adaptive` (ISA), which has its own HLS engine:
`inputstream`, `inputstream.adaptive.manifest_type=hls`, mime type
`application/vnd.apple.mpegurl`. The Nix package depends on
`inputstream-adaptive`; the add-on has a "Playback engine" setting to fall
back to ffmpeg, and falls back automatically if ISA is not enabled. Pinned by
`test_seek_fmp4_inputstream_adaptive` (passes) and
`test_seek_fmp4_kodi_ffmpeg` (expected failure, documents the upstream bug).

## Still unconfirmed against real YouTube - Hypothesis

This sandbox has no route to YouTube, so it is **not yet confirmed** that
YouTube's HLS formats are fMP4 rather than MPEG-TS. If they are TS the user's
seek failure may have another cause (for example segment URL expiry or
request headers). Plan: run the same checks against a real video where
network is allowed (idea: `YOK_LIVE=1` mode in the harness), and capture
`yt-dlp -F` output plus a `kodi.log` from a failing seek on the user's box.

## Other ideas considered

- Serve a single remuxed stream with a local ffmpeg: no seeking until
  downloaded; rejected.
- Build a DASH MPD for ISA from yt-dlp's DASH formats, as the official
  YouTube add-on does: needs sidx/index ranges that yt-dlp does not expose;
  larger change, revisit if HLS via ISA proves flaky on live YouTube.
- Newer ffmpeg in Kodi: not possible, Kodi pins its own ffmpeg.
