# The living-room UI (Slint on DRM/KMS) and its session supervisor.
{ lib, rustPlatform, pkg-config, makeWrapper, alsa-utils, fontconfig, libxkbcommon, libinput, udev, libgbm, libGL, seatd }:

rustPlatform.buildRustPackage {
  pname = "appliance-ui";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./build.rs
      ./src
      ./ui
      ./session.sh
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ pkg-config makeWrapper ];
  buildInputs = [ fontconfig libxkbcommon libinput udev libgbm libGL seatd ];

  postInstall = ''
    install -Dm755 session.sh $out/bin/appliance-ui-session
    # The focus tick is played with aplay.
    wrapProgram $out/bin/appliance-ui --prefix PATH : ${lib.makeBinPath [ alsa-utils ]}
  '';

  meta = {
    description = "Media appliance UI spike: Slint on DRM/KMS";
    license = lib.licenses.gpl3Only;
    mainProgram = "appliance-ui";
  };
}
