-- Lituus – Hyprland integration (Hyprland 0.55 and newer, Lua config)
-- Paste into ~/.config/hypr/hyprland.lua, or save next to it and add:
--   require("lituus")      -- for ~/.config/hypr/lituus.lua
-- Using hyprland.conf instead? See hyprland.conf in this folder.

local lituus = [[^(ch\.giosuel\.Lituus)$]]

-- The window floats in the middle at the size it's designed for, instead of tiling.
hl.window_rule({
    match = { class = lituus, title = "^(Lituus)$" },
    float = true,
    size = "1040 680",
    center = true,
})

-- The quick popup (left-click on the tray icon, or `lituus --quick`): floating, on every
-- workspace, in the top right corner under the bar. For a bar at the bottom, or to open it at the
-- pointer instead: move = { "cursor_x-(window_w*0.5)", "cursor_y+12" }.
hl.window_rule({
    match = { class = lituus, title = "^(Lituus quick)$" },
    float = true,
    pin = true,
    size = { 380, 560 },
    move = { "monitor_w-window_w-12", "52" },
})

-- Start in the background with Hyprland, so NetworkManager can ask for the passwords kept in
-- your keyring when it connects at login.
hl.on("hyprland.start", function()
    hl.exec_cmd("lituus --background")
end)

-- Open the window (of the one running in the background), the quick popup, or switch Wi-Fi.
hl.bind("SUPER + N", hl.dsp.exec_cmd("lituus"))
hl.bind("SUPER + SHIFT + N", hl.dsp.exec_cmd("lituus --quick"))
hl.bind("XF86WLAN", hl.dsp.exec_cmd("lituus --toggle-wifi"))
