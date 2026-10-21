-- Vinculum – Hyprland integration (Hyprland 0.55 and newer, Lua config)
-- Paste into ~/.config/hypr/hyprland.lua, or save next to it and add:
--   require("vinculum")      -- for ~/.config/hypr/vinculum.lua
-- Using hyprland.conf instead? See hyprland.conf in this folder.

local vinculum = [[^(ch\.giosuel\.Vinculum)$]]

-- The window floats in the middle at the size it's designed for, instead of tiling.
hl.window_rule({
    match = { class = vinculum },
    float = true,
    size = "1040 680",
    center = true,
})

-- Start in the background with Hyprland, so NetworkManager can ask for the passwords kept in
-- your keyring when it connects at login.
hl.on("hyprland.start", function()
    hl.exec_cmd("vinculum --background")
end)

-- Open the window (of the one running in the background).
hl.bind("SUPER + N", hl.dsp.exec_cmd("vinculum"))
