# Desktop session evidence — 2026-10-27 (Linux / Hyprland)

Environment: Arch Linux, Hyprland 0.56 (Lua dispatchers), two outputs
(eDP-1 1920×1080 @1.25, HDMI-A-1 @1.0), Rust 1.98, Slint 1.18.1,
release build of `implementation/full-v1`.

This is evidence for specific claims only. It is not full v1 acceptance.

## Verified on the real desktop

| Claim | Observation |
|---|---|
| Release binary starts a real window | `cognitive-popups hub` mapped a toplevel titled “Cognitive Popups”, 734×785 tiled, and 1040×780 when floated |
| Window is addressable by the compositor | after the fix, `app_id`/class is `cognitive-popups` (was empty, so window rules could not target it) |
| Content renders | all eight sections visible; a screenshot showed the plan page with timer, plan fields, results and the “no FSRS review” note |
| Tabs fit narrow tiling | replaced the fixed `TabWidget` with a 2×4 grid; all eight labels visible at 734 px, no clipping |
| UI is interactive | `Tab` moved focus (`Предметы` showed a focus ring), `Enter` switched the page and re-rendered its content |
| Explicit material path | `note --manual-source` rendered the piped fragment with the label “ИСТОЧНИК: ввод вручную” |
| Keyboard typing reaches the app | typed text appeared in the note's own-words field |
| No crash during interaction | process stayed in its event loop; no panic or stderr output |

## Fixed during this check

1. Empty Wayland `app_id` — the compositor saw `class: ""`, so window rules and
   class-based scripts could not address the app. Fixed by initialising the
   platform and calling `slint::set_xdg_app_id("cognitive-popups")` before the
   first visible window.
2. Clipped tab bar — eight `TabWidget` tabs did not fit a tiled 734 px window.
   Replaced with an adaptive 2×4 grid of buttons.
3. `checkable` buttons whose `checked` was bound to an expression could not be
   toggled by clicking; switched to `primary` highlighting with a direct
   `page = N` assignment.

## Not verified

- **Pointer clicks.** The test environment's `ydotool` daemon did not deliver
  button events (no `wl_pointer.button` in `WAYLAND_DEBUG`). Pointer *motion*
  was received by the window, so this is a test-tool limitation, not an observed
  application failure; nevertheless no pointer interaction is claimed.
- Global hotkey binding and focus return to the previous application.
- Real selection capture from a browser, PDF viewer or editor.
- Live provider request with a real API key.
- Windows desktop behavior.
- Any visual/aesthetic sign-off.

Screenshots were taken with `grim` during the session and are not committed
(binary, session-specific). The behaviours above are described so they can be
reproduced: start `hub`, press `Tab`/`Enter`, and observe page changes and the
focus ring; run `echo text | cognitive-popups note --manual-source` for the
explicit-material path.
