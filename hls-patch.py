

# --- appended by the NixOS packaging ---------------------------------------
# YouTube no longer offers any muxed audio+video format, so the original
# "b[height<=?720]/b" selector fails ("Could not resolve stream"). Pair a
# video-only and an audio-only HLS stream in a local master playlist instead;
# Kodi's ffmpeg demuxer plays that as one stream. Falls back to the original
# muxed lookup if the HLS formats are unavailable.
_get_stream_url_muxed = get_stream_url
# Served over loopback HTTP by the playlist server: Kodi treats a
# local .m3u8 file as a playlist rather than an HLS stream.
_MASTER_DIR = "@masterDir@"


def get_stream_url(video_id, max_height=@maxHeight@):
    url = WATCH_URL.format(video_id)
    hls = "[protocol^=m3u8]"
    # Highest resolution up to max_height, preferring H.264 (hardware decode)
    # among equals. Above 1080p YouTube only offers VP9/AV1.
    fmt = "bv*{p},ba{p}".format(p=hls)
    sort = "res:{},vcodec:avc1".format(max_height)
    p = _run_ytdlp(["-S", sort, "-f", fmt, "--get-url", "--no-warnings", url])
    lines = [l for l in p.stdout.strip().splitlines() if l.strip()]
    if p.returncode != 0 or len(lines) < 2 or not os.access(_MASTER_DIR, os.W_OK):
        return _get_stream_url_muxed(video_id, max_height)
    video, audio = lines[0], lines[1]
    path = os.path.join(_MASTER_DIR, "{}.m3u8".format(video_id))
    atomic_write_text(
        path,
        '#EXTM3U\n'
        '#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="a",NAME="audio",DEFAULT=YES,'
        'AUTOSELECT=YES,URI="{a}"\n'
        '#EXT-X-STREAM-INF:BANDWIDTH=4000000,AUDIO="a"\n'
        '{v}\n'.format(a=audio, v=video),
    )
    return "http://127.0.0.1:@port@/{}.m3u8".format(video_id)
