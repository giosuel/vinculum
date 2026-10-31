---
title: Installing
description: Packages for Arch (AUR), Debian and Ubuntu, Fedora and openSUSE, the AppImage, the Flatpak, Nix, and building from source.
---

Every release has packages for x86-64 and ARM64 Linux on its
[GitHub release page](https://github.com/giosuel/lituus/releases). They're built on Debian 12,
so they run on Debian 12, Ubuntu 22.04, and anything newer.

Besides NetworkManager, some features use packages you may want:

- **OpenVPN** needs NetworkManager's OpenVPN plugin: `networkmanager-openvpn` (Arch),
  `network-manager-openvpn` (Debian, Ubuntu), `NetworkManager-openvpn` (Fedora).
- **Choosing files** (certificates, `.conf` and `.ovpn` files) opens your desktop's file dialog
  through the XDG desktop portal. On Hyprland that's `xdg-desktop-portal-gtk` (or `-kde`) next to
  `xdg-desktop-portal-hyprland`. Without one, type the path instead.
- **Passwords "for me only"** go to GNOME Keyring, KWallet or another Secret Service keyring.

## Arch Linux (AUR)

```sh
paru -S lituus-bin   # the prebuilt binary
paru -S lituus       # or built from source
```

## Debian, Ubuntu

```sh
sudo apt install ./lituus_<version>-1_amd64.deb   # arm64.deb on ARM
```

## Fedora, openSUSE

```sh
sudo dnf install ./lituus-<version>-1.x86_64.rpm   # aarch64.rpm on ARM
```

## AppImage

Runs on most distributions without installing anything:

```sh
chmod +x Lituus-<version>-x86_64.AppImage
./Lituus-<version>-x86_64.AppImage
```

## Flatpak

A bundle for x86-64. The freedesktop runtime it needs comes from Flathub, so add Flathub first if
you haven't (`flatpak remote-add --if-not-exists --user flathub https://dl.flathub.org/repo/flathub.flatpakrepo`):

```sh
flatpak install --user ./Lituus-<version>-linux-x64.flatpak
flatpak run ch.giosuel.Lituus
```

In the sandbox, Lituus may talk to NetworkManager, your keyring, the notification daemon and
the tray, and read your colour files; nothing else of your home folder. A few things work
differently there:

- The settings are in `~/.var/app/ch.giosuel.Lituus/config/lituus/config.toml`.
- The hotspot can't list the devices connected to it.
- Lituus can't see whether NetworkManager's OpenVPN plugin is installed; without it, OpenVPN
  connections fail when they're saved or started. WireGuard needs no plugin.
- Certificates you pick in the editor should be in `~/.local/share/lituus/certificates`, where
  NetworkManager can read them.
- With Hyprland, start it with `exec-once = flatpak run ch.giosuel.Lituus --background`.

The full list is in [`packaging/flatpak/`](https://github.com/giosuel/lituus/tree/main/packaging/flatpak).

## Nix

The repository is a flake:

```sh
nix run github:giosuel/lituus            # try it
nix profile install github:giosuel/lituus
nix run github:giosuel/lituus/v0.3.0     # a particular release
```

On NixOS or with home-manager, add `github:giosuel/lituus` as an input and use its
`packages.${system}.default` or `overlays.default`. For OpenVPN, add
`networking.networkmanager.plugins = [ pkgs.networkmanager-openvpn ];`.

## Tarball

`Lituus-<version>-linux-x64.tar.gz` (or `-arm64`) holds the `lituus` binary, plus the desktop
file, AppStream metainfo and icon under `packaging/linux/`, for packagers.

## From source

You need [Rust](https://rustup.rs) and fontconfig's development files (`libfontconfig1-dev` on
Debian and Ubuntu, `fontconfig-devel` on Fedora).

```sh
git clone https://github.com/giosuel/lituus
cd lituus
cargo build --release
./target/release/lituus
```

To check your downloads against the `SHA256SUMS` attached to the release:

```sh
sha256sum -c --ignore-missing SHA256SUMS
```
