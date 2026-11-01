---
title: Displays
description: How Lituus sets up monitors on Hyprland, and where it keeps them.
---

The Displays page sets up your monitors through Hyprland: drag them into place, and set each one's
resolution, refresh rate, scale, rotation, mirroring, colour mode (HDR), bit depth and variable
refresh rate.

## Apply, then keep

Changes wait for **Apply**, which sets them live and asks whether to keep them. With no answer in
15 seconds the old settings come back, so a monitor that stays black fixes itself. **Keep** writes
them to Lituus's own file.

## Lituus's own file

Lituus never edits your Hyprland config. It writes `~/.config/hypr/lituus.lua`, which your config
loads with one line in `~/.config/hypr/hyprland.lua`:

```lua
require("lituus")
```

Until that line is there, the page says so and offers to copy it: changes then last only until
Hyprland restarts. Monitors are matched by make, model and serial (`desc:`), so a monitor keeps its
settings when it moves to another port.

On Hyprland 0.54 or older, configured with `hyprland.conf`, Lituus writes `lituus.conf` instead:

```ini
source = ~/.config/hypr/lituus.conf
```

## Workspaces and profiles

**Workspaces** binds workspaces 1 to 10 to a monitor, picks the one each monitor shows when it
turns on, and keeps workspaces when they're empty.

**Profiles** saves the current layout under a name ("Desk + TV") to use again. A profile that
switches by itself is applied when exactly its monitors are plugged in, while Lituus runs (in the
tray, or with `lituus --background`), and can run a command afterwards, for example
`pkill -SIGUSR2 waybar`. Profiles and workspaces are kept in `~/.config/lituus/displays.toml`.
