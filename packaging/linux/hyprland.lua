-- Cognitive Popups — Hyprland 0.56 Lua keybinds (opt-in, never installed).
--
-- Ryoku keeps ~/.config/hypr/user.lua across updates, so these five lines
-- belong there (or in any file user.lua dofile()s). Do not edit files under
-- /usr/share/ryoku or the generated hyprland.lua.
--
-- Remove or disable older Cognitive Popups/Cognitive Exoskeleton binds for the
-- same keys first: two clients cannot both receive Alt+W, and the legacy
-- cognitive-exoskeleton scripts take the key from the focused XWayland window.
--
-- Reload after editing: hyprctl reload   (or the Ryoku Hub reload button)

local home = os.getenv("HOME") or ""
local bin = home .. "/.local/bin/cognitive-popups"

hl.bind("ALT + E", hl.dsp.exec_cmd(bin .. " comment"))          -- Alt+E
hl.bind("ALT + W", hl.dsp.exec_cmd(bin .. " four-words"))       -- Alt+W
hl.bind("ALT + T", hl.dsp.exec_cmd(bin .. " task"))             -- Alt+T
hl.bind("CTRL + ALT + P", hl.dsp.exec_cmd(bin .. " prediction")) -- Ctrl+Alt+P
hl.bind("ALT + I", hl.dsp.exec_cmd(bin .. " session"))          -- Alt+I: the session window, not the panel

-- The five windows are meant to float above the reader, never to be tiled.
hl.window_rule({
    name = "cognitive-popups-float",
    match = { class = "^cognitive-popups$" },
    float = true,
})
