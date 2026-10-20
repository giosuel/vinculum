# Vinculum

A network manager for Linux desktops that don't come with one, such as Hyprland and Sway: everything
`nmtui` does, plus the settings depth of KDE's network panel, in a native app written in Rust with
[Slint](https://slint.dev). It drives NetworkManager over D-Bus.

**Status:** in development. The window shows your networks and connections live, but can't change them yet.

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

## Colours

Vinculum takes its colours from the first usable source in `~/.config/vinculum/config.toml`, and
recolours itself when that file changes, so it follows your wallpaper. Without a config file it
uses these two, then its built-in theme:

```toml
[theme]
builtin = "auto"   # or "dark" / "light": used when no source is usable

[[theme.sources]]
path = "~/.config/vinculum/colors.json"

[[theme.sources]]
path = "~/.config/ml4w/colors/colors.json"   # ML4W writes matugen's colours here
```

A source is a matugen-style `colors.json` (Material colour roles such as `primary` and `surface` as
hex strings). One with missing roles, or text too faint to read, is skipped.

## Testing

```sh
scripts/test.sh
```

runs every test, including the ones against a mock NetworkManager (python-dbusmock on a private
bus, so the real one is never touched). It needs `dbus-daemon` and python-dbusmock
(`apt install python3-dbusmock`, or `pip install python-dbusmock` in a venv with
`VINCULUM_PYTHON` pointing at its `python`), and fails rather than skipping them when they're
missing.

`scripts/mock-nm.sh COMMAND` runs any command against the same mock, filled with the networks from
the mockups, for example the debug tool that prints what Vinculum reads from NetworkManager:

```sh
cargo build -p vinculum-cli
scripts/mock-nm.sh target/debug/vinculum-cli list   # or: target/debug/vinculum-cli list, for the real one
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
