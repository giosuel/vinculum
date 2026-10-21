# Vinculum

A network manager for Linux desktops that don't come with one, such as Hyprland and Sway: everything
`nmtui` does, plus the settings depth of KDE's network panel, in a native app written in Rust with
[Slint](https://slint.dev). It drives NetworkManager over D-Bus.

**Status:** in development. The window opens, but it doesn't manage anything yet.

## Building

Needs Rust (the version in `rust-toolchain.toml`) and fontconfig's development files.

```sh
cargo build --release
./target/release/vinculum
```

The app ID is `ch.giosuel.Vinculum`, so Hyprland window rules can match it:

```
windowrule = float, class:^(ch\.giosuel\.Vinculum)$
```

## Versions

The version comes from the git tags (`scripts/version.sh`): `Cargo.toml` always says `0.0.0-dev`,
and release builds write the tag's version in with `scripts/set-version.sh`.

## Dependency versions

Every crate version in `Cargo.lock` must have been public for at least 14 days, so a freshly
published (possibly compromised) release can't slip in. `cargo xtask dep-age` checks this, and CI
runs it. Security fixes and explicit exceptions go in `xtask/dep-age-exceptions.txt`, each with its
reason.

## License

[AGPL-3.0-or-later](LICENSE)
