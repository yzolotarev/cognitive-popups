# The control panel is not the product's entry point

Date: 2026-10-27
Status: accepted by the author during implementation

## Context

The first full build shipped one 1040×780 window with eight tabs and opened it
when the binary was started without arguments. The five actions (Alt+E comment,
Alt+W Four Words, Alt+T task/attempt/feedback, Ctrl+Alt+P prediction, Alt+I
session) already had their own windows, but the default start made the product
read as "one big menu with tabs".

## Decision

- The tabbed window is a **control panel** (`cognitive-popups hub`,
  `settings`, `subjects`, `context`, `memory`, `session`).
- Starting the binary with no arguments prints the command list and opens no
  window.
- The installed menu entry is named «Cognitive Popups — управление» and runs
  `hub` explicitly, so a click on it is a deliberate request for the panel.
- There is no autostart entry: every action is a one-shot command bound to a
  hotkey, so nothing has to run in the background.
- The five action windows stay separate and are never routed through the panel.
  Alt+I (`session`) opens the panel directly on the session page because the
  session plan is a long-form surface, not a popup.

## Consequences

- Nothing opens at login and nothing opens by accident.
- The panel can grow (memory, experiment, settings) without turning the product
  into a menu, because it is never the first thing a user sees.
- The session page remains inside the panel; splitting it into its own window
  stays an option if the panel still feels heavy in use.
