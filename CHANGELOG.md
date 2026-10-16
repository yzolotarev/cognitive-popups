# Changelog

## 1.0.0 — 2026-10-16

Mature release, built from 26 days of real use (126 notes, 85 hypotheses) and
Justin Sung's method: AI gives raw material, the reader does the structuring.

**Reliability**
- A hotkey pressed again raises its window, or replaces it if it is gone; windows
  closed by the compositor no longer linger.
- Esc on the step window starts nothing; an early end can cancel the step.
- Tests never touch real state; the ledger schema is never lowered; three backups.
- The service starts without the model bridge; one state directory (XDG).

**Honest feedback**
- A hypothesis is checked against the text only; questions and vague guesses are
  caught in code; the answer stays on screen with what holds and where to look.
- Missed hypotheses come back after 2, 5, 12 and 30 days, without the text.
- AI no longer solves exercises, hands over the missing piece, or builds a
  paragraph from wrong guesses; cue words are content words.
- Stale prompt overrides are noticed and ignored.
- One-click Sung self-check at the end of a step.

**Comfort and visuals**
- Six everyday keys; `Ctrl+Alt+P` bound in the repo config; `Ctrl+Q` removed;
  `Alt+Shift+K` opens every action.
- Weekly overview (`cog stats`), «на бумагу» handover, session card SVG.
- One theme and one reduced-motion rule; HUD, Universe timer and sand sound off
  by default; one-line errors and a no-AI draft of the four words.

**Six months without updates**
- `cog doctor`, `cog maintain` (weekly timer), one model config with API key and
  timeout, newer PyGObject compatibility, broken state files set aside.
