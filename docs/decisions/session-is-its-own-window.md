# Alt+I is a session window of its own

Date: 2026-10-27
Status: accepted by the author during implementation
Supersedes: the last paragraph of `hub-is-a-control-panel.md`

## Context

Alt+I used to open the control panel on a session tab. That made the fifth main
action the only one without its own window: Alt+E, Alt+W, Alt+T and Ctrl+Alt+P
each opened a purpose-built surface, while the session lived in the same tabbed
window that also holds subjects, memory, experiment and settings. The timer and
the plan — the two things a session is made of — shared a tab bar with
configuration screens, and a launch of the panel was indistinguishable from a
launch of a session.

## Decision

- `cognitive-popups session` opens a separate `SessionWindow`
  (`crates/ui/ui/note.slint`, controller `crates/app/src/session.rs`).
- The window holds, in this order: the timer and the session state, the session
  picker, the subject picker with its goal, the plan under the timer, and the
  explicit actions (save the plan, start, resume, pause, finish).
- The plan is the main body of the window; the plan fields are the model's
  draft, always editable, never started by implication.
- The window owns its own draft (`("draft","session")`), its own single-instance
  lock (`session.lock`) and its own provider route (`provider/plan`, falling
  back to `provider/default`).
- On open it offers the newest Active session (recovered to paused) and
  otherwise the newest Paused one; a freshly generated AI draft opens
  immediately under the timer.
- The hub loses the session tab and every session field. It stays a control
  panel: subjects, context, memory, check, history, experiment, settings.

## Consequences

- Every one of the five actions has its own window and its own hotkey; the
  panel never stands in for an action.
- Closing the session window does not silently pause or start anything: the
  stored state is what it is, and the next open recovers an Active session to
  paused so time is never counted while nobody looks.
- `InstanceGuard` must know the surface (`INSTANCE_WINDOWS`), otherwise the
  window refuses to open — the whitelist is part of the surface definition now.
- The hub no longer knows whether a session is running; the experiment's
  `in_session` flag is derived from stored session state instead of UI state.
