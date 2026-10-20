# Hyprland

Vinculum's window has the class (Wayland app ID) `ch.giosuel.Vinculum`, so window rules can match
it. It's a normal window rather than a popup, so you decide where it goes.

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

## A polkit agent

NetworkManager asks polkit before changing connections that are shared with all users. On
Hyprland that needs a polkit agent running, for example:

```ini
exec-once = systemctl --user start hyprpolkitagent
```

Connections stored only for your user don't need it. When an action does, Vinculum says so
instead of failing silently.
