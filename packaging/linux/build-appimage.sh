#!/usr/bin/env bash
# Packs the release binary as an AppImage for this machine's architecture:
#   packaging/linux/build-appimage.sh   →   target/dist/Vinculum-<version>-<x86_64|aarch64>.AppImage
#
# An AppImage only runs where glibc is at least as new as on the build machine, so the release
# workflow runs this on Debian 12: the result runs on Debian 12, Ubuntu 22.04 and later.
# Needs curl and file, and the same system libraries as a normal build.
set -euo pipefail
cd "$(dirname "$0")/../.."
export PATH="$HOME/.cargo/bin:$PATH"

LINUXDEPLOY_VERSION=1-alpha-20251107-1
arch=$(uname -m)
case "$arch" in
  x86_64) LINUXDEPLOY_SHA256=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d ;;
  aarch64) LINUXDEPLOY_SHA256=620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff ;;
  *) echo "build-appimage.sh: no linuxdeploy for $arch" >&2; exit 1 ;;
esac

cargo build --release --locked -p vinculum
target_dir=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)

# linuxdeploy collects the libraries the binary links against and packs the AppImage.
tools="$target_dir/appimage-tools"
linuxdeploy="$tools/linuxdeploy-$LINUXDEPLOY_VERSION-$arch.AppImage"
if [ ! -x "$linuxdeploy" ]; then
  mkdir -p "$tools"
  curl -sSfL -o "$linuxdeploy.part" \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/$LINUXDEPLOY_VERSION/linuxdeploy-$arch.AppImage"
  echo "$LINUXDEPLOY_SHA256  $linuxdeploy.part" | sha256sum -c --quiet
  chmod +x "$linuxdeploy.part"
  mv "$linuxdeploy.part" "$linuxdeploy"
fi

appdir="$target_dir/appimage/Vinculum.AppDir"
rm -rf "$appdir"
install -Dm755 "$target_dir/release/vinculum" "$appdir/usr/bin/vinculum"
install -Dm644 packaging/linux/ch.giosuel.Vinculum.desktop -t "$appdir/usr/share/applications"
install -Dm644 packaging/linux/ch.giosuel.Vinculum.metainfo.xml -t "$appdir/usr/share/metainfo"
install -Dm644 packaging/linux/icons/ch.giosuel.Vinculum.svg -t "$appdir/usr/share/icons/hicolor/scalable/apps"
install -Dm644 crates/app/ui/fonts/*-LICENSE.txt -t "$appdir/usr/share/licenses/vinculum"

mkdir -p target/dist
out="$PWD/target/dist/Vinculum-$version-$arch.AppImage"
rm -f "$out"
# Without FUSE: containers and CI runners often lack it.
export APPIMAGE_EXTRACT_AND_RUN=1
export LDAI_OUTPUT="$out" OUTPUT="$out"
"$linuxdeploy" --appdir "$appdir" \
  --desktop-file "$appdir/usr/share/applications/ch.giosuel.Vinculum.desktop" \
  --icon-file "$appdir/usr/share/icons/hicolor/scalable/apps/ch.giosuel.Vinculum.svg" \
  --output appimage
echo "$out"
