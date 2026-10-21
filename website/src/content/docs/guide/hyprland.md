---
title: Hyprland
description: Starting Vinculum with Hyprland, window rules, a keybind and the polkit agent.
---

Vinculum's window has the class (Wayland app ID) `ch.giosuel.Vinculum`, so window rules can match
it. It's a normal window rather than a popup, so you decide where it goes.

## Start it with Hyprland

Passwords you keep "for me only" are in your keyring, and NetworkManager asks Vinculum for them
whenever it connects, including right after you log in. So start it in the background:

```ini
exec-once = vinculum --background
```

It stays out of sight until NetworkManager needs a password. Running `vinculum` opens the window
of the one that's already running, and closing the window leaves it running in the background.

## Float it

```ini
# ~/.config/hypr/hyprland.conf
windowrule = float, class:^(ch\.giosuel\.Vinculum)$
windowrule = size 1040 680, class:^(ch\.giosuel\.Vinculum)$
windowrule = center, class:^(ch\.giosuel\.Vinculum)$
```

## Open it with a key

```ini
bind = SUPER, N, exec, vinculum
```

With Vinculum running in the background, this opens its window instead of starting a second one.

## A polkit agent

NetworkManager asks polkit before changing connections that are shared with all users. On
Hyprland that needs a polkit agent running, for example:

```ini
exec-once = systemctl --user start hyprpolkitagent
```

Connections stored only for your user don't need it. When an action does, Vinculum says so
instead of failing silently.
