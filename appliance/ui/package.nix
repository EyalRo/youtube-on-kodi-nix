# The living-room UI (Slint on DRM/KMS) and its session supervisor.
{ lib, rustPlatform, pkg-config, makeWrapper, imagemagick, dejavu_fonts, alsa-utils, fontconfig, libxkbcommon, libinput, udev, libgbm, libGL, seatd
, debug ? false # compile the debug logs in (see ../package.nix)
}:

rustPlatform.buildRustPackage {
  pname = "appliance-ui";
  version = "0.3.1";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      # The UI's own workspace...
      ./Cargo.toml
      ./Cargo.lock
      ./build.rs
      ./src
      ./ui
      ./session.sh
      # ...and the applog crate it depends on (a path dependency, so its source must be present
      # for the build; applog lives one level up and is self-contained).
      ../crates/applog
    ];
  };

  # cargo builds in the ui/ subdir of the combined source; the UI is its own workspace.
  cargoRoot = "ui";
  buildAndTestSubdir = "ui";

  cargoLock.lockFile = ./Cargo.lock;

  preBuild = lib.optionalString debug ''
    export RUSTFLAGS="$RUSTFLAGS --cfg appliance_debug"
  '';

  nativeBuildInputs = [ pkg-config makeWrapper imagemagick ];
  buildInputs = [ fontconfig libxkbcommon libinput udev libgbm libGL seatd ];

  postInstall = ''
    # The logo as an 8-bit alpha mask for fbsplash (and a PNG for the boot splash).
    mkdir -p $out/share/appliance-ui
    export FONTCONFIG_FILE=${fontconfig.out}/etc/fonts/fonts.conf
    magick -size 1400x300 xc:black -font ${dejavu_fonts}/share/fonts/truetype/DejaVuSans-Bold.ttf \
      -pointsize 190 -gravity center -fill white -annotate +0+0 Shmetflix \
      -depth 8 gray:$out/share/appliance-ui/logo-1400x300.gray
    magick -size 1400x300 xc:none -font ${dejavu_fonts}/share/fonts/truetype/DejaVuSans-Bold.ttf \
      -pointsize 190 -gravity center -fill "#e50914" -annotate +0+0 Shmetflix $out/share/appliance-ui/logo.png
    # session.sh lives in the ui/ subdir of the combined source.
    install -Dm755 ui/session.sh $out/bin/appliance-ui-session
    # The focus tick is played with aplay.
    wrapProgram $out/bin/appliance-ui --prefix PATH : ${lib.makeBinPath [ alsa-utils ]}
  '';

  meta = {
    description = "Media appliance UI spike: Slint on DRM/KMS";
    license = lib.licenses.gpl3Only;
    mainProgram = "appliance-ui";
  };
}
