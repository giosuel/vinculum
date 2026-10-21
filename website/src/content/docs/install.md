---
title: Installing
description: Packages for Arch (AUR), Debian and Ubuntu, Fedora and openSUSE, the AppImage, and building from source.
---

Every release has packages for x86-64 and ARM64 Linux on its
[GitHub release page](https://github.com/giosuel/vinculum/releases). They're built on Debian 12,
so they run on Debian 12, Ubuntu 22.04, and anything newer.

## Arch Linux (AUR)

```sh
paru -S vinculum-bin   # the prebuilt binary
paru -S vinculum       # or built from source
```

## Debian, Ubuntu

```sh
sudo apt install ./vinculum_<version>-1_amd64.deb   # arm64.deb on ARM
```

## Fedora, openSUSE

```sh
sudo dnf install ./vinculum-<version>-1.x86_64.rpm   # aarch64.rpm on ARM
```

## AppImage

Runs on most distributions without installing anything:

```sh
chmod +x Vinculum-<version>-x86_64.AppImage
./Vinculum-<version>-x86_64.AppImage
```

## Tarball

`Vinculum-<version>-linux-x64.tar.gz` (or `-arm64`) holds the `vinculum` binary, plus the desktop
file, AppStream metainfo and icon under `packaging/linux/`, for packagers.

## From source

You need [Rust](https://rustup.rs) and fontconfig's development files (`libfontconfig1-dev` on
Debian and Ubuntu, `fontconfig-devel` on Fedora).

```sh
git clone https://github.com/giosuel/vinculum
cd vinculum
cargo build --release
./target/release/vinculum
```

To check your downloads against the `SHA256SUMS` attached to the release:

```sh
sha256sum -c --ignore-missing SHA256SUMS
```
