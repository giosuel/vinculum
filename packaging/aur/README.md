# AUR packages

- `lituus`: builds the tagged source with cargo.
- `lituus-bin`: installs the release tarball (`Lituus-<version>-linux-<x64|arm64>.tar.gz`).

The release workflow pushes both for every stable tag (no `-` in it): it sets `pkgver` from the
tag, refreshes the checksums, test-builds the package and pushes `PKGBUILD` and `.SRCINFO`. So the
`pkgver` here can stay old, and the `.SRCINFO` files aren't kept in this repository.

Before the first release, the owner:

1. creates both packages on the AUR under their account (a first push of `PKGBUILD` and
   `.SRCINFO`, made with `packaging/aur/test.sh`), and
2. adds the AUR SSH key as the repository secret `AUR_SSH_PRIVATE_KEY`.

`packaging/aur/test.sh` builds both packages in a clean Arch container with namcap, installs them
and writes their `.SRCINFO`. For a version that isn't released yet, it builds from the local
checkout.
