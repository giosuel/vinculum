# Lituus for Nix, built from a source tree (the flake passes this checkout).
{
  lib,
  rustPlatform,
  pkg-config,
  fontconfig,
  libxkbcommon,
  wayland,
  libGL,
  xorg,
  src,
  version,
}:

let
  # Loaded at run time (dlopen) by Slint's window backend and renderer, so they go on the
  # binary's RPATH rather than in LD_LIBRARY_PATH, which would leak into what it starts.
  runtimeLibs = [
    wayland
    libxkbcommon
    libGL
    xorg.libX11
    xorg.libXcursor
    xorg.libXrandr
    xorg.libXi
  ];
in
rustPlatform.buildRustPackage {
  pname = "lituus";
  inherit src version;

  cargoLock.lockFile = ../../Cargo.lock;

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [
    fontconfig
    libxkbcommon
  ];

  # The repository says 0.0.0-dev; the release workflow writes the tag's version in the same way.
  # After the patch phase, where the vendored crates are checked against the unchanged Cargo.lock.
  postPatch = "patchShebangs scripts";
  preBuild = "scripts/set-version.sh ${version}";

  cargoBuildFlags = [
    "-p"
    "lituus"
  ];
  # The unit tests; the integration tests need D-Bus mocks of NetworkManager and BlueZ, and a
  # keyring.
  cargoTestFlags = [
    "-p"
    "lituus"
    "-p"
    "lituus-nm"
    "-p"
    "lituus-bt"
    "-p"
    "lituus-keyring"
    "--lib"
    "--bins"
  ];

  postInstall = ''
    install -Dm644 packaging/linux/ch.giosuel.Lituus.desktop -t $out/share/applications
    install -Dm644 packaging/linux/ch.giosuel.Lituus.metainfo.xml -t $out/share/metainfo
    install -Dm644 packaging/linux/icons/ch.giosuel.Lituus.svg -t $out/share/icons/hicolor/scalable/apps
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath runtimeLibs} $out/bin/lituus
  '';

  meta = {
    description = "Settings for Hyprland and other desktops without them: network, Bluetooth, sound and displays";
    homepage = "https://lituus.giosuel.ch/";
    license = lib.licenses.agpl3Plus;
    mainProgram = "lituus";
    platforms = lib.platforms.linux;
  };
}
