---
title: Hyprland
description: Starting Vinculum with Hyprland, window rules, keybinds, the tray icon and the polkit agent.
---

Vinculum's window has the class (Wayland app ID) `ch.giosuel.Vinculum`, so window rules can match
it. It's a normal window rather than a popup, so you decide where it goes. Ready-made files are in
the repository's [`extras/`](https://github.com/giosuel/vinculum/tree/main/extras) folder.

## The Lua config (Hyprland 0.55 and newer)

```lua
-- ~/.config/hypr/hyprland.lua
local vinculum = [[^(ch\.giosuel\.Vinculum)$]]

hl.window_rule({
    match = { class = vinculum, title = "^(Vinculum)$" },
    float = true,
    size = "1040 680",
    center = true,
})

-- The quick popup, in the top right corner under the bar.
hl.window_rule({
    match = { class = vinculum, title = "^(Vinculum quick)$" },
    float = true,
    pin = true,
    size = { 380, 560 },
    move = { "monitor_w-window_w-12", "52" },
})

hl.on("hyprland.start", function()
    hl.exec_cmd("vinculum --background")
end)

hl.bind("SUPER + N", hl.dsp.exec_cmd("vinculum"))
hl.bind("SUPER + SHIFT + N", hl.dsp.exec_cmd("vinculum --quick"))
hl.bind("XF86WLAN", hl.dsp.exec_cmd("vinculum --toggle-wifi"))
```

For a bar at the bottom, or to open the popup at the pointer, use
`move = { "cursor_x-(window_w*0.5)", "cursor_y+12" }` instead.

## hyprland.conf

```ini
# ~/.config/hypr/hyprland.conf, Hyprland 0.53 and newer
windowrule = float on, match:class ^(ch\.giosuel\.Vinculum)$, match:title ^(Vinculum)$
windowrule = size 1040 680, match:class ^(ch\.giosuel\.Vinculum)$, match:title ^(Vinculum)$
windowrule = center on, match:class ^(ch\.giosuel\.Vinculum)$, match:title ^(Vinculum)$
windowrule = float on, match:class ^(ch\.giosuel\.Vinculum)$, match:title ^(Vinculum quick)$
windowrule = pin on, match:class ^(ch\.giosuel\.Vinculum)$, match:title ^(Vinculum quick)$
windowrule = size 380 560, match:class ^(ch\.giosuel\.Vinculum)$, match:title ^(Vinculum quick)$
windowrule = move (monitor_w-window_w-12) 52, match:class ^(ch\.giosuel\.Vinculum)$, match:title ^(Vinculum quick)$

exec-once = vinculum --background
bind = SUPER, N, exec, vinculum
bind = SUPER SHIFT, N, exec, vinculum --quick
bind = , XF86WLAN, exec, vinculum --toggle-wifi
```

On Hyprland 0.48 to 0.52 the rules are written the other way round, for example
`windowrule = float, class:^(ch\.giosuel\.Vinculum)$, title:^(Vinculum)$` (all of them are in `extras/hyprland.conf`); before 0.48 they're `windowrulev2`.

## Why start it in the background

Passwords you keep "for me only" are in your keyring, and NetworkManager asks Vinculum for them
whenever it connects, including right after you log in. `vinculum --background` stays out of
sight until NetworkManager needs a password. Running `vinculum` (from the keybind) opens the window
of the one that's already running, and closing the window leaves it running in the background.

## The tray icon and Waybar

While Vinculum runs, its icon is in Waybar's `tray` module (or any other StatusNotifierItem tray),
showing the signal of the network you're on. Right-click it for the menu (Wi-Fi, networks, VPNs,
airplane mode), left-click it for the quick popup. With a tray, closing the window keeps Vinculum
running there. The tray icon, that behaviour and the notifications can be turned off in Settings.

Waybar's own `network` module can open the popup too:

```json
"network": {
    "on-click": "vinculum --quick",
    "on-click-right": "vinculum"
}
```

## A polkit agent

NetworkManager asks polkit before changing connections that are shared with all users. On
Hyprland that needs a polkit agent running, for example:

```ini
exec-once = systemctl --user start hyprpolkitagent
```

Connections stored only for your user don't need it. When an action does, Vinculum says so
instead of failing silently.
