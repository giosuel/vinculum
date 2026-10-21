#!/usr/bin/env bash
# Packs the release binary with its desktop file, metainfo and icon:
#   scripts/package-tarball.sh 0.1.0 x64   →   target/dist/Vinculum-0.1.0-linux-x64.tar.gz
# The files keep their paths from the repository, so vinculum-bin installs them like the
# source package does. Run after `cargo build --release`.
set -euo pipefail
cd "$(dirname "$0")/.."
version="${1:?usage: package-tarball.sh VERSION ARCH}"
arch="${2:?usage: package-tarball.sh VERSION ARCH}"
name="Vinculum-$version-linux-$arch"
rm -rf "target/dist/$name"
mkdir -p "target/dist/$name/packaging/linux/icons"
cp target/release/vinculum README.md LICENSE "target/dist/$name/"
strip "target/dist/$name/vinculum"
cp packaging/linux/ch.giosuel.Vinculum.desktop packaging/linux/ch.giosuel.Vinculum.metainfo.xml \
  "target/dist/$name/packaging/linux/"
cp packaging/linux/icons/ch.giosuel.Vinculum.svg "target/dist/$name/packaging/linux/icons/"
# The fonts built into the binary come with their licence (SIL Open Font License).
mkdir -p "target/dist/$name/crates/app/ui/fonts"
cp crates/app/ui/fonts/*-LICENSE.txt "target/dist/$name/crates/app/ui/fonts/"
tar -C target/dist -czf "target/dist/$name.tar.gz" "$name"
rm -rf "target/dist/$name"
echo "target/dist/$name.tar.gz"
