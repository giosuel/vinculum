# Flatpak

`ch.giosuel.Vinculum.yml` builds the app on the freedesktop 26.08 runtime with the Rust SDK
extension. Slint draws everything itself, so it needs neither the GNOME nor the KDE runtime. Builds
are offline, so every crate in `Cargo.lock` is listed in `cargo-sources.json`.

| File | What it is |
| --- | --- |
| `ch.giosuel.Vinculum.yml` | Manifest. Builds the local checkout (`type: dir`). |
| `cargo-sources.json` | Crate sources. Regenerate with `update-cargo-sources.sh` whenever `Cargo.lock` changes; CI's `deps` job fails until you do (`check-cargo-sources.py`). |
| `flathub-manifest.sh` | Prints the manifest for Flathub: building a release tag instead of the checkout, with its version written in. |
| `test.sh` | Builds with `org.flatpak.Builder` (what Flathub uses) in a container, runs Flathub's linter, and checks that the window opens on headless sway (`smoke.sh`). |

The release workflow builds the bundle on every packaging PR (as a dry run) and attaches
`Vinculum-<version>-linux-x64.flatpak` to each release. Install it with
`flatpak install --user Vinculum-<version>-linux-x64.flatpak`; the runtime comes from Flathub.

## Permissions

| Permission | Why |
| --- | --- |
| `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc`, `--device=dri` | The window, with GPU rendering |
| `--system-talk-name=org.freedesktop.NetworkManager` | Everything the app shows and changes. NetworkManager still asks polkit for anything that affects other users |
| `--talk-name=org.freedesktop.secrets`, `org.kde.kwalletd6`, `org.kde.kwalletd5` | Passwords kept "for me only", in GNOME Keyring or KWallet |
| `--talk-name=org.freedesktop.Notifications` | Connected, disconnected and failed notifications |
| `--talk-name=org.kde.StatusNotifierWatcher` | Registering the tray icon with the bar (Waybar, KDE, …) |
| `--filesystem=xdg-config/ml4w/colors:ro`, `--filesystem=xdg-config/vinculum:ro` | The colours to follow: what ML4W's matugen writes, and your own `colors.json`. Read-only |
| `--filesystem=xdg-data/vinculum:create` | `~/.local/share/vinculum/certificates`, where imported VPN files' certificates and keys go, so NetworkManager (outside the sandbox) can read them |

The app owns its own bus name (`ch.giosuel.Vinculum`, for the single instance), which needs no
permission. It needs no network access of its own and can't read your home folder beyond the
folders above. To drop one, for example the KWallet names on a GNOME desktop:
`flatpak override --user --no-talk-name=org.kde.kwalletd6 --no-talk-name=org.kde.kwalletd5 ch.giosuel.Vinculum`.

## What works differently in the sandbox

- **The settings file** is `~/.var/app/ch.giosuel.Vinculum/config/vinculum/config.toml`, not
  `~/.config/vinculum/config.toml` (Flatpak gives every app its own config folder). The colour
  sources in it still point at your real `~/.config/...` files.
- **Hotspot clients:** the list of connected devices comes from NetworkManager's lease file in
  `/var/lib/NetworkManager`, which the sandbox can't see, so the list stays empty. The hotspot
  itself works.
- **OpenVPN:** the sandbox can't see whether NetworkManager's OpenVPN plugin is installed, so the
  app doesn't check: without the plugin, NetworkManager refuses the connection when it's saved or
  started. WireGuard is built into NetworkManager.
- **Certificates chosen in the editor** come through the file chooser portal. One outside the
  folders above arrives as a `/run/user/<uid>/doc/…` path, which NetworkManager can't read. Put
  certificates in `~/.local/share/vinculum/certificates` first. (Imported `.ovpn` files are
  copied there already.)
- **Starting in the background** with Hyprland: `exec-once = flatpak run ch.giosuel.Vinculum --background`.

## Submitting to Flathub

1. Fork [flathub/flathub](https://github.com/flathub/flathub) and branch from `new-pr`.
2. Add the Flathub manifest and the crate sources:

    ```sh
    packaging/flatpak/flathub-manifest.sh v0.3.0 > ../flathub/ch.giosuel.Vinculum.yml
    cp packaging/flatpak/cargo-sources.json ../flathub/
    ```

3. Open a PR against `new-pr`, titled `Add ch.giosuel.Vinculum`.
4. After it's accepted, Flathub creates `flathub/ch.giosuel.Vinculum`. Update that repository for
   each release, with the same two commands.

Flathub's linter (`test.sh` runs it) still reports, and a submission needs:

- **Exceptions** for `finish-args-unnecessary-xdg-config-ml4w-ro-access`,
  `…-xdg-config-vinculum-ro-access` and `…-xdg-data-vinculum-create-access`. The linter flags any
  `xdg-config/<folder>` access, but these are the point: following another app's colours, and
  putting certificates where NetworkManager can read them. Ask for them in the submission PR.
- **Screenshots** in the metainfo (`metainfo-missing-screenshots`), hosted at a stable URL.
- **A `<release>` entry**: the release workflow writes one for each stable tag, so a manifest from
  `flathub-manifest.sh` for a stable tag has it.
- **The app ID's domain**: Flathub checks that `https://giosuel.ch` answers. Inside the test
  container it can't reach this server (`appid-url-not-reachable`), which is only the container.

Expect review questions about the system bus access to NetworkManager (it's what the app is for)
and the keyring names.
