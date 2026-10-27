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
| Real primary-selection capture | `wl-copy --primary …` then `cognitive-popups capture` printed `kind: selection` with the exact text and empty diagnostics |
| Capture → Four Words window | `four-words` opened with the captured text, labelled “Выделение” and the configured endpoint/model disclosure |
| Missing provider configuration is handled | activating the generate button with no model showed the configuration error in the window; nothing was sent and the app stayed alive |
| Alt+T window from a real selection | `task` opened “Микроупражнение · Alt+T” with the captured text, the source kind, the own-attempt field and the “no attempt on close” footer |
| Ctrl+Alt+P window | `prediction` opened “Свой прогноз · Ctrl+Alt+P” with the hypothesis prompt before any model call |
| Focus comes back after an explicit window | with kitty active, `note --manual-source` took focus; `Escape` closed it and `kitty` was active again |
| All eight hub sections render | `Занятие`, `Предметы`, `Контекст`, `Память`, `Проверка`, `История`, `Опыт`, `Настройки` were each opened and screenshotted; `context`/`memory`/`settings` also via their CLI entry points |
| Empty states explain themselves | history and MRT report show a short explanation instead of a blank box |
| Pointer switching between sections | a real left click on the `Настройки` and `Предметы` buttons changed the shown section |
| Pointer editing and saving a subject | clicking the name and goal fields, typing, then clicking `Сохранить предмет` wrote `subject` revision 1 (`Математика для работы` / `Считать маржу без ошибок`) and showed the zero-weights status |
| Weight validation in the GUI | entering `{"four_words":1,"flashcards":2}` and saving left revision 1 unchanged and showed “неизвестное имя механики…” |
| Note saved end to end | `note --manual-source`, typed text, `Ctrl+Enter` appended a `user_text` journal event and the process exited |
| Alt+I session end to end | “Свой план”, objective and success criteria typed, `Сохранить план`, `Начать / продолжить`, `Пауза`, `Начать / продолжить`, `Завершить работу`; the workspace row moved `Paused → Active → Paused → Active → Completed` and `reviews` stayed 0 |
| Exact-answer check, inexact answer | expected `цена продажи`, answered `цена продажи!`: the attempt was stored as `Incorrect` with no review, and the workspace gained no schedule |
| Exact-answer check, exact answer, FSRS off | expected `себестоимость`, answered `себестоимость` independently: stored as `Correct`, status “FSRS применён к 0 компонентам”, still 0 reviews, because FSRS for that unit was never enabled |
| Check verdict is visible | the verdict appeared above the question field; before the fix it was rendered below the last button and fell outside the window |
| No window on plain start | `cognitive-popups` with no arguments printed the command list and mapped no window (`hyprctl -j clients` listed none) |
| Control panel is explicit | `cognitive-popups hub`/`session` opened the window titled «Cognitive Popups — управление» |
| Interrupted session recovery | after killing the hub with an `Active` session, the restart showed `пауза` in both the header and the selector, kept the plan and did not mark the session completed |

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
4. The check-answer `TextEdit` collapsed to one line and its placeholder was
   clipped; it now has a minimum height and a shorter prompt.
5. A label still said “вне активного Alt+I” and the settings text pointed at a
   tab named “Эксперимент”; both now use the shipped wording (“занятия”, «Опыт»).
6. Session, self-assessment and observation statuses were printed as Rust enum
   values (`Paused`, `Mastered`, `[Correct]`); they are now Russian words.
7. The session selector kept the status it had when the list was loaded, so it
   could say “пауза” while the header said “идёт”; the list is refreshed after
   every save/action. An empty objective no longer leaves a dangling “·”.
8. On restart the selector showed the pre-recovery `идёт` while the header
   showed `пауза`; the lists are rebuilt after draft recovery so both agree.

## Not verified

- Global hotkey binding (the compositor already binds Alt+E/W/T/I on this
  machine, so the exact combinations were deliberately not remapped).
- Real selection capture from a browser, PDF viewer or editor.
- Live provider request with a real API key.
- Windows desktop behavior.
- Any visual/aesthetic sign-off.

Left clicks are injected with `ydotool click 0xC0`; `0x00` produced no button
event in this build. A tiled window may be re-laid out between two clicks (for
example when a popup closes), so the geometry was re-read from
`hyprctl -j clients` before each click, or the window was pinned by floating it.

Screenshots were taken with `grim` during the session and are not committed
(binary, session-specific). The behaviours above are described so they can be
reproduced: start `hub`, press `Tab`/`Enter`, and observe page changes and the
focus ring; run `echo text | cognitive-popups note --manual-source` for the
explicit-material path.
