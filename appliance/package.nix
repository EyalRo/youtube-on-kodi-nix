# The appliance's binaries (mediad, ytsync) from the Cargo workspace.
# `debug = true` compiles the debug logs in (RUSTFLAGS --cfg appliance_debug) and lifts the
# default log filter to debug; without it they are not in the binaries at all.
{ lib, rustPlatform, debug ? false }:

rustPlatform.buildRustPackage {
  pname = "media-appliance";
  version = "0.3.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./crates
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  preBuild = lib.optionalString debug ''
    export RUSTFLAGS="$RUSTFLAGS --cfg appliance_debug"
  '';

  # Read by the NixOS module (updateCheck checks every 5 minutes on a debug build).
  passthru.debug = debug;

  # Builds the whole workspace, runs its tests, installs the two binaries.
  meta = {
    description = "Media appliance back end: library daemon and YouTube download job";
    license = lib.licenses.gpl2Plus;
    mainProgram = "mediad";
  };
}
