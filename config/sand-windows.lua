-- User-owned application overrides. No shell settings or shipped files.
hl.window_rule({
    name = "cognitive-transparent-surfaces",
    match = { class = "^cognitive-(popup|window|shadow|sand|input|flash|ask|focus-timer|hud|panel)$" },
    float = true,
    no_anim = true,
    no_shadow = true,
    no_blur = true,
    opaque = false,
    border_size = 0,
    tag = "+hyprglass_disabled",
})
hl.window_rule({
    name = "cognitive-sand-over-fullscreen",
    match = { class = "^cognitive-sand$" },
    float = true,
    pin = true,
})
-- Text cards must stay readable when the page under them has focus
-- (Ryoku dims inactive windows to 0.94; 10.10.2026, owner's decision).
hl.window_rule({
    name = "cognitive-opaque-cards",
    match = { class = "^cognitive-(popup|window|flash|input|ask)$" },
    opacity = "1.0 override 1.0 override",
})
