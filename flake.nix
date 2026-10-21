{
  description = "Vinculum: Wi-Fi, wired and VPN connections for Hyprland and other Linux desktops";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # Cargo.toml says 0.0.0-dev and the build has no git, so the version names the commit,
      # like a development build's: 0.0.0-dev+g1a2b3c4.
      version = "0.0.0-dev+g${self.shortRev or self.dirtyShortRev or "unknown"}";

      vinculum =
        pkgs:
        pkgs.callPackage ./packaging/nix/package.nix {
          inherit version;
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./rust-toolchain.toml
              ./crates
              ./xtask
              ./packaging/linux
              ./scripts
            ];
          };
        };
    in
    {
      packages = forAllSystems (pkgs: {
        vinculum = vinculum pkgs;
        default = self.packages.${pkgs.stdenv.hostPlatform.system}.vinculum;
      });

      overlays.default = final: _prev: { vinculum = vinculum final; };

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.vinculum ];
          packages = with pkgs; [
            clippy
            rustfmt
            rust-analyzer
          ];
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              wayland
              libxkbcommon
              libGL
              xorg.libX11
              xorg.libXcursor
              xorg.libXrandr
              xorg.libXi
            ]
          );
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
