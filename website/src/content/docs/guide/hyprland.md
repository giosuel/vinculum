---
title: Hyprland
description: Starting Lituus with Hyprland, window rules, keybinds, the tray icon and the polkit agent.
---

Lituus's window has the class (Wayland app ID) `ch.giosuel.Lituus`, so window rules can match
it. It's a normal window rather than a popup, so you decide where it goes. Ready-made files are in
the repository's [`extras/`](https://github.com/giosuel/lituus/tree/main/extras) folder.

## The Lua config (Hyprland 0.55 and newer)

```lua
-- ~/.config/hypr/hyprland.lua
local lituus = [[^(ch\.giosuel\.Lituus)$]]

hl.window_rule({
    match = { class = lituus, title = "^(Lituus)$" },
    float = true,
    size = "1200 760",
    center = true,
})

-- The quick popup, in the top right corner under the bar.
hl.window_rule({
    match = { class = lituus, title = "^(Lituus quick)$" },
    float = true,
    pin = true,
    size = { 380, 600 },
    move = { "monitor_w-window_w-12", "52" },
})

hl.on("hyprland.start", function()
    hl.exec_cmd("lituus --background")
end)

hl.bind("SUPER + N", hl.dsp.exec_cmd("lituus"))
hl.bind("SUPER + SHIFT + N", hl.dsp.exec_cmd("lituus --quick"))
hl.bind("XF86WLAN", hl.dsp.exec_cmd("lituus --toggle-wifi"))
```

For a bar at the bottom, or to open the popup at the pointer, use
`move = { "cursor_x-(window_w*0.5)", "cursor_y+12" }` instead.

## hyprland.conf

```ini
# ~/.config/hypr/hyprland.conf, Hyprland 0.53 and newer
windowrule = float on, match:class ^(ch\.giosuel\.Lituus)$, match:title ^(Lituus)$
windowrule = size 1200 760, match:class ^(ch\.giosuel\.Lituus)$, match:title ^(Lituus)$
windowrule = center on, match:class ^(ch\.giosuel\.Lituus)$, match:title ^(Lituus)$
windowrule = float on, match:class ^(ch\.giosuel\.Lituus)$, match:title ^(Lituus quick)$
windowrule = pin on, match:class ^(ch\.giosuel\.Lituus)$, match:title ^(Lituus quick)$
windowrule = size 380 600, match:class ^(ch\.giosuel\.Lituus)$, match:title ^(Lituus quick)$
windowrule = move (monitor_w-window_w-12) 52, match:class ^(ch\.giosuel\.Lituus)$, match:title ^(Lituus quick)$

exec-once = lituus --background
bind = SUPER, N, exec, lituus
bind = SUPER SHIFT, N, exec, lituus --quick
bind = , XF86WLAN, exec, lituus --toggle-wifi
```

On Hyprland 0.48 to 0.52 the rules are written the other way round, for example
`windowrule = float, class:^(ch\.giosuel\.Lituus)$, title:^(Lituus)$` (all of them are in `extras/hyprland.conf`); before 0.48 they're `windowrulev2`.

## Why start it in the background

Passwords you keep "for me only" are in your keyring, and NetworkManager asks Lituus for them
whenever it connects, including right after you log in. `lituus --background` stays out of
sight until NetworkManager needs a password. Running `lituus` (from the keybind) opens the window
of the one that's already running, and closing the window leaves it running in the background.

## The tray icon and Waybar

While Lituus runs, its icon is in Waybar's `tray` module (or any other StatusNotifierItem tray),
showing the signal of the network you're on. Right-click it for the menu (Wi-Fi, networks, VPNs,
airplane mode), left-click it for the quick popup. With a tray, closing the window keeps Lituus
running there. The tray icon, that behaviour and the notifications can be turned off in Settings.

Waybar's own `network` module can open the popup too:

```json
"network": {
    "on-click": "lituus --quick",
    "on-click-right": "lituus"
}
```

## A polkit agent

NetworkManager asks polkit before changing connections that are shared with all users. On
Hyprland that needs a polkit agent running, for example:

```ini
exec-once = systemctl --user start hyprpolkitagent
```

Connections stored only for your user don't need it. When an action does, Lituus says so
instead of failing silently.
