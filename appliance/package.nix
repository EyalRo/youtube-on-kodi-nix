# The appliance's binaries (mediad, ytsync) from the Cargo workspace.
{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "media-appliance";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./crates
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  # Builds the whole workspace, runs its tests, installs the two binaries.
  meta = {
    description = "Media appliance back end: library daemon and YouTube download job";
    license = lib.licenses.gpl2Plus;
    mainProgram = "mediad";
  };
}
