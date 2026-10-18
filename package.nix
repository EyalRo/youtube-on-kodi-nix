# Used as: pkgs.kodi.packages.callPackage ./package.nix { }
{ lib
, buildKodiAddon
, yt-dlp
, inputstream-adaptive
, port ? 8099
, maxHeight ? 2160
, masterDir ? "/run/youtube-on-kodi"
  # Defaults of the library/games settings (all changeable in the add-on).
, libraryDir ? ""        # where channels are exported as TV shows ("" = add-on profile)
, gamesDir ? ""          # ROM folder, one subfolder per system ("" = games off)
, refreshMinutes ? 0     # background refresh interval, 0 = off
}:

assert lib.elem maxHeight [ 360 480 720 1080 1440 2160 ];

buildKodiAddon {
  pname = "youtube-on-kodi";
  namespace = "plugin.video.myyoutubers";
  version = "1.7.0";

  src = ./addon;

  # Seeking in fragmented-MP4 HLS needs inputstream.adaptive; Kodi's bundled
  # ffmpeg breaks the video after a seek.
  propagatedBuildInputs = [ inputstream-adaptive ];

  # Build-time configuration; see the placeholders in resources/lib/kanyt.py.
  postPatch = ''
    substituteInPlace resources/lib/kanyt.py \
      --replace-fail '@ytdlp@' '${yt-dlp}/bin/yt-dlp' \
      --replace-fail '@masterDir@' '${masterDir}' \
      --replace-fail '@port@' '${toString port}' \
      --replace-fail '@maxHeight@' '${toString maxHeight}'
    substituteInPlace resources/settings.xml \
      --replace-fail 'values="360|480|720|1080|1440|2160" default="1080"' \
                     'values="360|480|720|1080|1440|2160" default="${toString maxHeight}"'
    substituteInPlace resources/settings.xml \
      --replace-fail 'label="Library folder for exported channels (empty = add-on profile)" default=""' \
                     'label="Library folder for exported channels (empty = add-on profile)" default="${lib.escapeXML libraryDir}"' \
      --replace-fail 'label="ROM folder (one subfolder per system, e.g. NES, Genesis)" default=""' \
                     'label="ROM folder (one subfolder per system, e.g. NES, Genesis)" default="${lib.escapeXML gamesDir}"' \
      --replace-fail 'option="int" default="0" />
</settings>' \
                     'option="int" default="${toString refreshMinutes}" />
</settings>'
  '';
}
