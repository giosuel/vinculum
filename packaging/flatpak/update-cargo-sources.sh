#!/usr/bin/env bash
# Regenerate cargo-sources.json from Cargo.lock. Flathub builds offline, so every
# crate is listed as a source. Run this whenever Cargo.lock changes.
#
#   packaging/flatpak/update-cargo-sources.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
engine="$(command -v podman || command -v docker)"

# Pinned flatpak-builder-tools generator, verified by checksum.
GEN_URL=https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/74697c75b630d7330e77250fc13cb5ea688d9479/cargo/flatpak-cargo-generator.py
GEN_SHA256=0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
curl -fsSL "$GEN_URL" -o "$work/flatpak-cargo-generator.py"
echo "$GEN_SHA256  $work/flatpak-cargo-generator.py" | sha256sum --check --quiet
cp "$root/Cargo.lock" "$work/"

"$engine" run --rm -v "$work:/work:Z" -w /work docker.io/library/python:3.13-slim sh -euc '
  pip install --quiet --root-user-action=ignore "aiohttp>=3.9.5,<4" "PyYAML>=6.0.2,<7" "tomlkit>=0.13.3,<1"
  python3 -I flatpak-cargo-generator.py Cargo.lock -o cargo-sources.json
'
cp "$work/cargo-sources.json" "$here/cargo-sources.json"
echo "Wrote $here/cargo-sources.json ($(grep -c '"type": "archive"' "$here/cargo-sources.json") crates)"
