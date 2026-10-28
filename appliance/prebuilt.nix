# The appliance's binaries as CI compiled them (.github/workflows/ci.yml publishes them as the
# `latest` release), patched to run against this system's libraries. Nothing is compiled here.
#
#   (pkgs.callPackage ./appliance/prebuilt.nix { }) src   # -> { appliance, appliance-ui }
#
# `src` is the unpacked release tarball: backend/bin/* and ui/{bin/*,session.sh}. These are
# drop-in replacements for packages.appliance and packages.appliance-ui.
{ lib, stdenv, autoPatchelfHook, makeWrapper, imagemagick, dejavu_fonts, alsa-utils
, fontconfig, libxkbcommon, libinput, udev, libgbm, libGL, seatd, mpv-unwrapped }:

src: {
  appliance = stdenv.mkDerivation {
    pname = "media-appliance-prebuilt";
    version = "latest";
    inherit src;
    nativeBuildInputs = [ autoPatchelfHook ];
    # `play` links libmpv in-process (the `mpv` crate); mpv-unwrapped's .out ships libmpv.so.2.
    buildInputs = [ stdenv.cc.cc.lib mpv-unwrapped ];
    # CI compiles with the debug logs in, so this counts as a debug build (see ../module.nix).
    passthru.debug = true;
    installPhase = "install -Dm755 -t $out/bin backend/bin/*";
    meta.mainProgram = "mediad";
  };

  appliance-ui = stdenv.mkDerivation {
    pname = "appliance-ui-prebuilt";
    version = "latest";
    inherit src;
    nativeBuildInputs = [ autoPatchelfHook makeWrapper imagemagick ];
    buildInputs = [ stdenv.cc.cc.lib fontconfig libxkbcommon libinput udev libgbm libGL seatd ];
    # Libraries Slint opens at run time rather than linking.
    runtimeDependencies = [ libGL libgbm libinput libxkbcommon seatd udev ];
    installPhase = ''
      install -Dm755 -t $out/bin ui/bin/*
      install -Dm755 ui/session.sh $out/bin/appliance-ui-session
      mkdir -p $out/share/appliance-ui
      export FONTCONFIG_FILE=${fontconfig.out}/etc/fonts/fonts.conf
      magick -size 1400x300 xc:black -font ${dejavu_fonts}/share/fonts/truetype/DejaVuSans-Bold.ttf \
        -pointsize 190 -gravity center -fill white -annotate +0+0 Shmetflix \
        -depth 8 gray:$out/share/appliance-ui/logo-1400x300.gray
      magick -size 1400x300 xc:none -font ${dejavu_fonts}/share/fonts/truetype/DejaVuSans-Bold.ttf \
        -pointsize 190 -gravity center -fill "#e50914" -annotate +0+0 Shmetflix $out/share/appliance-ui/logo.png
      wrapProgram $out/bin/appliance-ui --prefix PATH : ${lib.makeBinPath [ alsa-utils ]}
    '';
    meta.mainProgram = "appliance-ui";
  };
}
