# Used as: pkgs.kodi.packages.callPackage ./package.nix { }
{ lib
, buildKodiAddon
, yt-dlp
, port ? 8099
, maxHeight ? 2160
, masterDir ? "/run/youtube-on-kodi"
}:

assert lib.elem maxHeight [ 360 480 720 1080 1440 2160 ];

buildKodiAddon {
  pname = "youtube-on-kodi";
  namespace = "plugin.video.myyoutubers";
  version = "1.7.0";

  src = ./addon;

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
  '';
}
