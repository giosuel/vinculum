# Vinculum

A network manager for Linux desktops that don't come with one, such as **Hyprland** and **Sway**.
Everything `nmtui` does, plus the settings depth of KDE's network panel, in a native app.

- **Connect** to Wi-Fi, wired and WireGuard networks, and disconnect, forget or share them.
- **Edit every setting**: security (including WPA Enterprise), IPv4 and IPv6, DNS, routes, MAC
  address, metered connections, auto-connect priority.
- **Fits a tiling desktop**: keyboard driven, quick to open, a normal window that window rules can
  float and place, and a tray icon.
- **Your colours**: Vinculum can take its colours from your wallpaper palette (matugen, as used by
  ML4W) and recolours itself when you change wallpaper.

Vinculum drives [NetworkManager](https://networkmanager.dev) over D-Bus, so it works with the
connections you already have, and alongside `nmcli` and `nmtui`.

!!! warning "In development"
    Vinculum is at its very start: the window opens, but it doesn't manage connections yet. The
    list above is what the first versions are being built to do.

## Requirements

- Linux with NetworkManager running.
- A polkit agent (for example `hyprpolkitagent`) for changes that need administrator rights, such
  as connections shared with all users.

See [Installing](installing.md) for the packages.
