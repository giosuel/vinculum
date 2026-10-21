---
title: Hyprland
description: Starting Vinculum with Hyprland, window rules, a keybind and the polkit agent.
---

Vinculum's window has the class (Wayland app ID) `ch.giosuel.Vinculum`, so window rules can match
it. It's a normal window rather than a popup, so you decide where it goes. Ready-made files are in
the repository's [`extras/`](https://github.com/giosuel/vinculum/tree/main/extras) folder.

## The Lua config (Hyprland 0.55 and newer)

```lua
-- ~/.config/hypr/hyprland.lua
hl.window_rule({
    match = { class = [[^(ch\.giosuel\.Vinculum)$]] },
    float = true,
    size = "1040 680",
    center = true,
})

hl.on("hyprland.start", function()
    hl.exec_cmd("vinculum --background")
end)

hl.bind("SUPER + N", hl.dsp.exec_cmd("vinculum"))
```

## hyprland.conf

```ini
# ~/.config/hypr/hyprland.conf, Hyprland 0.53 and newer
windowrule = match:class ^(ch\.giosuel\.Vinculum)$, float on
windowrule = match:class ^(ch\.giosuel\.Vinculum)$, size 1040 680
windowrule = match:class ^(ch\.giosuel\.Vinculum)$, center on

exec-once = vinculum --background
bind = SUPER, N, exec, vinculum
```

On Hyprland 0.48 to 0.52 the rules are written the other way round, for example
`windowrule = float, class:^(ch\.giosuel\.Vinculum)$`; before 0.48 they're `windowrulev2`.

## Why start it in the background

Passwords you keep "for me only" are in your keyring, and NetworkManager asks Vinculum for them
whenever it connects, including right after you log in. `vinculum --background` stays out of
sight until NetworkManager needs a password. Running `vinculum` (from the keybind) opens the window
of the one that's already running, and closing the window leaves it running in the background.

## A polkit agent

NetworkManager asks polkit before changing connections that are shared with all users. On
Hyprland that needs a polkit agent running, for example:

```ini
exec-once = systemctl --user start hyprpolkitagent
```

Connections stored only for your user don't need it. When an action does, Vinculum says so
instead of failing silently.
