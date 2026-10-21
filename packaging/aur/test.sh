#!/usr/bin/env bash
# Builds both AUR packages in a clean Arch container, lints them with namcap, installs them and
# refreshes their .SRCINFO files.   packaging/aur/test.sh
#
# Until the PKGBUILDs' version is released on GitHub, the sources come from the local checkout
# instead, with the binary tarball made like the release workflow makes it.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
engine="$(command -v podman || command -v docker)"
version=$(sed -n 's/^pkgver=//p' "$here/vinculum/PKGBUILD")
localsrc=/dev/null
if ! curl -fsSIL -o /dev/null "https://github.com/giosuel/vinculum/releases/download/v$version/Vinculum-$version-linux-x64.tar.gz" 2>/dev/null; then
  echo "==> v$version is not released: building from the local checkout"
  localsrc="$(mktemp -d)"
  trap 'rm -rf "$localsrc"' EXIT
  # Tracked and new files, as they are now (deleted ones are skipped).
  (cd "$root" && git ls-files -co --exclude-standard -z |
    tar --ignore-failed-read -czf "$localsrc/vinculum-$version.tar.gz" --null -T - --transform "s,^,vinculum-$version/,")
fi
# shellcheck disable=SC2016 # the script runs inside the container
"$engine" run --rm -v "$here:/aur:Z" -v "$localsrc:/local:ro,Z" -e version="$version" docker.io/archlinux:base-devel bash -euo pipefail -c '
  pacman -Syu --noconfirm --needed namcap >/dev/null
  useradd -m builder
  echo "builder ALL=(ALL) NOPASSWD: ALL" > /etc/sudoers.d/builder
  for pkg in vinculum vinculum-bin; do
    echo "==> $pkg"
    cp -r "/aur/$pkg" "/home/builder/$pkg"
    if [ -d /local ]; then
      if [ "$pkg" = vinculum ]; then
        cp "/local/vinculum-$version.tar.gz" /home/builder/vinculum/
      else
        # The release tarball, from the source package just built (scripts/package-tarball.sh).
        src="/home/builder/vinculum/src/vinculum-$version"
        (cd "$src" && scripts/package-tarball.sh "$version" x64 >/dev/null)
        cp "$src/target/dist/Vinculum-$version-linux-x64.tar.gz" /home/builder/vinculum-bin/
      fi
    fi
    chown -R builder: "/home/builder/$pkg"
    cd "/home/builder/$pkg"
    namcap PKGBUILD
    su builder -c "makepkg --syncdeps --noconfirm --cleanbuild" >makepkg.log 2>&1 || { tail -40 makepkg.log; exit 1; }
    grep -E "test result|Finished" makepkg.log || true
    su builder -c "makepkg --printsrcinfo" > "/aur/$pkg/.SRCINFO"
    namcap ./*.pkg.tar.zst
    pacman -U --noconfirm ./"$pkg"-[0-9]*.pkg.tar.zst >/dev/null
    vinculum --version
    pacman -Ql "$pkg" | grep -v "/$"
    pacman -R --noconfirm "$pkg" >/dev/null
  done
'
