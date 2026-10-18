# Estuary Rows: Kodi's Estuary skin plus a first "Rows" home menu entry whose
# widgets are the Youtube On Kodi row directories (rows.py). Built from the
# Estuary in the given Kodi package so only the changes live in this repo; the
# patch fails loudly if a Kodi update changes the files it touches.
# Used as: pkgs.kodi.packages.callPackage ./skin.nix { }
# `stressWidgets` swaps in 15 widgets (one per system) for the Python crash
# experiment in docs/wiki/Home-rows; never use it for real.
{ buildKodiAddon, kodi, stressWidgets ? false }:

buildKodiAddon {
  pname = "skin-estuary-rows${if stressWidgets then "-stress" else ""}";
  version = "4.0.0.1";
  namespace = "skin.estuary-rows";

  dontUnpack = true;

  installPhase = ''
    dir=$out/share/kodi/addons/skin.estuary-rows
    mkdir -p $dir
    cp -r ${kodi}/share/kodi/addons/skin.estuary/. $dir/
    chmod -R u+w $dir
    patch -d $dir -p1 < ${./skin/estuary-rows.patch}
    cp ${if stressWidgets then ./skin/Includes_Rows_stress.xml else ./skin/Includes_Rows.xml} $dir/xml/Includes_Rows.xml
  '';
}
