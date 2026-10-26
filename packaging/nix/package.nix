# Vinculum for Nix, built from a source tree (the flake passes this checkout).
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
  pname = "vinculum";
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
    "vinculum"
  ];
  # The unit tests; the integration tests need D-Bus mocks of NetworkManager and BlueZ, and a
  # keyring.
  cargoTestFlags = [
    "-p"
    "vinculum"
    "-p"
    "vinculum-nm"
    "-p"
    "vinculum-bt"
    "-p"
    "vinculum-keyring"
    "--lib"
    "--bins"
  ];

  postInstall = ''
    install -Dm644 packaging/linux/ch.giosuel.Vinculum.desktop -t $out/share/applications
    install -Dm644 packaging/linux/ch.giosuel.Vinculum.metainfo.xml -t $out/share/metainfo
    install -Dm644 packaging/linux/icons/ch.giosuel.Vinculum.svg -t $out/share/icons/hicolor/scalable/apps
  '';

  postFixup = ''
    patchelf --add-rpath ${lib.makeLibraryPath runtimeLibs} $out/bin/vinculum
  '';

  meta = {
    description = "Manage Wi-Fi, wired and VPN connections on Linux desktops";
    homepage = "https://vinculum.giosuel.ch/";
    license = lib.licenses.agpl3Plus;
    mainProgram = "vinculum";
    platforms = lib.platforms.linux;
  };
}
