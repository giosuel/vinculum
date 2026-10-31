#!/usr/bin/env bash
# Builds the Flatpak from the local checkout in a container, runs Flathub's linter on the
# manifest and the result, and checks that the app starts.
#
#   packaging/flatpak/test.sh
#
# Needs podman (runs privileged, for flatpak-builder's sandbox). Runtimes are cached in the
# "lituus-flatpak" volume. The bundle ends up in target/dist/. The build itself (a few GB) goes
# to $FLATPAK_OUT, by default .flatpak-builder in the checkout.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
out="${FLATPAK_OUT:-$root/.flatpak-builder}"
podman volume exists lituus-flatpak || podman volume create lituus-flatpak >/dev/null
mkdir -p "$root/target/dist" "$out"

# shellcheck disable=SC2016 # the script runs inside the container
podman run --rm --privileged -v lituus-flatpak:/var/lib/flatpak -v "$root:/src:Z" -v "$out:/out:Z" \
  docker.io/library/fedora:43 bash -euo pipefail -c '
  dnf -q -y install flatpak ostree dbus-daemon sway >/dev/null 2>&1
  flatpak remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
  # flatpak-builder cannot install runtimes from inside its own sandbox.
  flatpak install -y --noninteractive flathub org.flatpak.Builder \
    org.freedesktop.Platform//26.08 org.freedesktop.Sdk//26.08 org.freedesktop.Sdk.Extension.rust-stable//26.08 >/dev/null
  export XDG_RUNTIME_DIR=/run/user/0 && mkdir -p -m 700 "$XDG_RUNTIME_DIR"
  cd /src/packaging/flatpak
  out=/out # state, build and repo must share a filesystem
  # org.flatpak.Builder (what Flathub uses) asks the host flatpak about runtimes over the session bus.
  builder() { dbus-run-session -- flatpak run --command="$1" org.flatpak.Builder "${@:2}" 2>&1 | { grep -v "^dbus-daemon" || true; }; }

  echo "==> lint manifest"
  # Findings are reported, not fatal: some permissions need an exception from Flathub (README.md).
  builder flatpak-builder-lint manifest ch.giosuel.Lituus.yml || true
  echo "==> build"
  builder flatpak-builder --force-clean --disable-rofiles-fuse --repo="$out/repo" --state-dir="$out/state" \
    "$out/build" ch.giosuel.Lituus.yml >/tmp/build.log 2>&1 || { tail -40 /tmp/build.log; exit 1; }
  grep -E "Finished|Exporting" /tmp/build.log || true
  echo "==> lint repo"
  builder flatpak-builder-lint repo "$out/repo" || true
  echo "==> run"
  flatpak remote-delete --force local 2>/dev/null || true
  flatpak remote-add --no-gpg-verify local "$out/repo"
  flatpak install -y --noninteractive --reinstall local ch.giosuel.Lituus >/dev/null
  flatpak run ch.giosuel.Lituus --version
  setcap -r /usr/bin/sway 2>/dev/null || true # file caps make sway refuse to run as root
  dbus-run-session -- bash /src/packaging/flatpak/smoke.sh 2>&1 | { grep -v "^dbus-daemon" || true; }
  flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
    "$out/repo" /src/target/dist/ch.giosuel.Lituus.flatpak ch.giosuel.Lituus
  ls -l /src/target/dist/ch.giosuel.Lituus.flatpak
'
