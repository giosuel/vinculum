# Nix

`flake.nix` (repository root) builds Vinculum from the checkout with
`packaging/nix/package.nix`. It provides `packages.{x86_64,aarch64}-linux.default`, an overlay
and a dev shell. CI builds it on every PR.

## Use the flake

```sh
nix run github:giosuel/vinculum                         # try it
nix profile install github:giosuel/vinculum             # install it
nix run github:giosuel/vinculum/v0.3.0                  # a release
nix develop                                              # cargo, clippy, the libraries
```

On NixOS or with home-manager, add the flake as an input and use
`inputs.vinculum.packages.${pkgs.stdenv.hostPlatform.system}.default`, or apply
`overlays.default`. NetworkManager itself comes from your system
(`networking.networkmanager.enable = true`); for OpenVPN add
`networking.networkmanager.plugins = [ pkgs.networkmanager-openvpn ];`.

`vinculum --version` says `0.0.0-dev+g<commit>`: the flake has no git history to find the
release tag in, so the version names the commit it was built from.

The libraries Slint loads at run time (Wayland, xkbcommon, OpenGL, X11) go on the binary's RPATH,
so they don't leak into programs Vinculum starts.

## Updating nixpkgs

`flake.lock` pins nixos-unstable. Like every dependency, the pinned revision should be at least
two weeks old: pick a channel release from https://releases.nixos.org/?prefix=nixos/unstable/
and run `nix flake lock --override-input nixpkgs github:NixOS/nixpkgs/<rev>`, then set
`original` back to `nixos-unstable` in `flake.lock`.

## nixpkgs

Not submitted. nixpkgs takes packages with a real user base, and its
[contributing guide](https://github.com/NixOS/nixpkgs/blob/master/CONTRIBUTING.md) asks for
AI-assisted work to be disclosed with an `Assisted-by:` commit trailer. When it's time, the package
needs a `fetchFromGitHub` source for a release tag and a `cargoHash` instead of the checkout, a
maintainer in `meta.maintainers`, and a PR adding `pkgs/by-name/vi/vinculum/package.nix`.
