#!/usr/bin/env bash
# Prints the manifest for the Flathub repository: the same as ch.giosuel.Vinculum.yml, but
# building the given release tag instead of the local checkout, with the tag's version written in
# (the repository itself says 0.0.0-dev, and Flathub builds without git).
#
#   packaging/flatpak/flathub-manifest.sh v0.3.0 > ../flathub/ch.giosuel.Vinculum.yml
set -euo pipefail
tag="${1:?usage: flathub-manifest.sh TAG}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
url=https://github.com/giosuel/vinculum.git
commit=$(git ls-remote "$url" "refs/tags/$tag^{}" | cut -f1)
[[ -n "$commit" ]] || commit=$(git ls-remote "$url" "refs/tags/$tag" | cut -f1)
[[ -n "$commit" ]] || { echo "error: tag $tag not found on $url" >&2; exit 1; }

awk -v url="$url" -v tag="$tag" -v commit="$commit" -v version="${tag#v}" '
  /# flathub-manifest.sh swaps/ { next }
  /- cargo --offline build/ {
    indent = substr($0, 1, index($0, "-") - 1)
    print indent "- scripts/set-version.sh " version
  }
  /- type: dir/ {
    indent = substr($0, 1, index($0, "-") - 1)
    print indent "- type: git"
    print indent "  url: " url
    print indent "  tag: " tag
    print indent "  commit: " commit
    getline; getline   # drop path: and skip:
    next
  }
  { print }
' "$here/ch.giosuel.Vinculum.yml"
