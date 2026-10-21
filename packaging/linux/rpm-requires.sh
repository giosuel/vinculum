#!/usr/bin/env bash
# Prints the .rpm's requirements for a binary as an inline TOML table, for
# `cargo generate-rpm --set-metadata "requires = $(packaging/linux/rpm-requires.sh BINARY)"`.
#
# cargo-generate-rpm's own scan writes glibc requirements with a "[WEAK]" marker that no RPM
# distribution provides, so the package couldn't be installed. Instead: every library the binary
# links by soname, glibc at the newest symbol version it uses, the libraries it loads at runtime,
# and NetworkManager.
set -euo pipefail
binary="${1:?usage: rpm-requires.sh BINARY}"
reqs=('"NetworkManager" = "*"' '"libxkbcommon.so.0()(64bit)" = "*"' '"libEGL.so.1()(64bit)" = "*"')
glibc=$(objdump -T "$binary" | grep -o 'GLIBC_[0-9][0-9.]*' | sort -uV | tail -1)
reqs+=("\"libc.so.6($glibc)(64bit)\" = \"*\"")
while read -r lib; do
  case "$lib" in
    ld-linux*|libc.so.6) ;; # the loader comes with glibc
    *) reqs+=("\"$lib()(64bit)\" = \"*\"") ;;
  esac
done < <(objdump -p "$binary" | awk '$1 == "NEEDED" { print $2 }' | sort -u)
printf '{ %s }\n' "$(IFS=,; echo "${reqs[*]}" | sed 's/,/, /g')"
