# Used as: pkgs.kodi.packages.callPackage ./package.nix { }
{ buildKodiAddon
, fetchFromGitHub
, yt-dlp
, port ? 8099
, maxHeight ? 2160
, masterDir ? "/run/youtube-on-kodi"
}:

buildKodiAddon {
  pname = "youtube-on-kodi";
  namespace = "plugin.video.myyoutubers";
  version = "1.6.5";

  src = fetchFromGitHub {
    owner = "XPDevs";
    repo = "Youtube-On-Kodi";
    rev = "954b94f3ee9cbae337f55403270331118d20ecf3";
    hash = "sha256-1FQ+L6WBtkstQh08DiD/AGOjArb2ucRkSLYG2RTiT3U=";
  };

  postPatch = ''
    # The add-on runs `python <yt-dlp>`, but nixpkgs' yt-dlp is a bash wrapper.
    substituteInPlace resources/lib/kanyt.py \
      --replace-fail 'cmd = [find_python(), find_ytdlp(), "--no-update"]' \
                     'cmd = ["${yt-dlp}/bin/yt-dlp", "--no-update"]'

    # Items are IsPlayable, so Kodi waits for the plugin to resolve them.
    # Starting playback with Player.play() leaves the handle unresolved and
    # Kodi pops up "playback failed" over the working video.
    substituteInPlace default.py \
      --replace-fail 'player.play(stream_url, li)' \
                     'xbmcplugin.setResolvedUrl(HANDLE, True, li)'

    # YouTube no longer offers muxed audio+video formats; pair HLS streams.
    cat ${./hls-patch.py} >> resources/lib/kanyt.py
    substituteInPlace resources/lib/kanyt.py \
      --replace-fail '@masterDir@' '${masterDir}' \
      --replace-fail '@port@' '${toString port}' \
      --replace-fail '@maxHeight@' '${toString maxHeight}'
  '';
}
