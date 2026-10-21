# Vinculum

A network manager for Linux desktops that don't come with one, such as Hyprland and Sway: everything
`nmtui` does, plus the settings depth of KDE's network panel, in a native app written in Rust with
[Slint](https://slint.dev). It drives NetworkManager over D-Bus.

**Status:** in development. It connects to Wi-Fi (open, password-protected and hidden networks),
wired and VPN connections, keeps passwords in your keyring, and edits connections: general
settings, Wi-Fi, security, IPv4 and IPv6 (addresses, DNS, routes) and Ethernet. Enterprise
(802.1X), WireGuard, OpenVPN and hotspots come next.

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

## Passwords and the background mode

When you join a network, Vinculum asks where to keep its password:

- **For me only** (the default): in your keyring (GNOME Keyring, KeePassXC or anything else that
  speaks the Secret Service, or KWallet), and the connection is yours alone. No admin password
  needed. Without a keyring, NetworkManager keeps the password instead.
- **All users**: NetworkManager keeps it for everyone; changing it needs an admin password (polkit).
- **Ask each time**: not saved at all.

"For me only" passwords live in your keyring, so NetworkManager asks Vinculum for them whenever it
connects, including at login. Start Vinculum in the background so it's there to answer:

```
exec-once = vinculum --background
```

It runs without a window until NetworkManager needs a password; `vinculum` (from a keybind, say)
then opens the window of the running one, and closing the window leaves it running. Passwords that
nm-applet or GNOME Settings put in the keyring are found too.

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
missing. With `gnome-keyring` installed, the keyring tests also run against a real one on a
private bus.

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
