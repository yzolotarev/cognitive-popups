# Cognitive Exoskeleton v1 — working guide

This is an implementation in progress, not a signed-off v1 release.

## Start and install

Run `cognitive-exoskeleton hub` (or launch without arguments). `--help` is a
non-GUI diagnostic and works without a desktop. Linux release packages include
`install.sh`: run `sh install.sh` for a user-local launcher; add `--autostart`
only if wanted. Windows packages include `install.ps1` (user-local Start menu
shortcut; optional `-Autostart`). Neither installer edits your hotkeys.

Linux needs the desktop runtime libraries used by Slint (Wayland/X11, EGL,
fontconfig), `wl-clipboard` on Wayland or `xclip` on X11, and an unlocked Secret
Service for API keys. No insecure key-file fallback is available. Windows uses
Credential Manager and clipboard fallback; automatic selection capture and
global hotkey acceptance are not yet verified on a Windows desktop.

The application uses the OS-local state directory under
`cognitive-exoskeleton-v1` (Linux: usually
`~/.local/state/cognitive-exoskeleton-v1`; Windows: the project-local AppData
directory). Earlier development databases under `cognitive-exoskeleton` are
left untouched, not silently imported. The historical app is never launched
or modified by this runtime.

## Settings and privacy

In **Настройки**, choose the default, context or plan provider; enter the full
OpenAI-compatible `https://…/v1/chat/completions` endpoint, model ID and your API
key. HTTP is allowed only for a loopback/local provider. Keys are stored in the
system credential store, not SQLite or diagnostics. A blank key keeps the
existing key for that exact endpoint. Changing endpoint does not reuse another
endpoint's key. Redirects are rejected; failed requests are not retried silently.

Network requests occur only after an explicit send/plan button. Context sends
the visible material and own thought. Planning sends the selected subject,
material, that subject's previous plans/self-reports and code-check evidence.
These fields are personal data. No screen/audio recording or clipboard watcher
is running. Local raw texts are retained; no unapproved TTL or auto-deletion
policy has been introduced.

## Working loop

1. **Предметы**: create a subject with your own long-term goal and context.
   Optional preferences are integer weights in a JSON object; automatic
   reweighting by lessons is not enabled without the outstanding policy decision.
2. **Контекст**: capture a fragment explicitly or paste/type material. Clipboard
   fallback is labelled unverified/stale-possible; confirm the displayed text
   yourself. Your thought and AI output have different fields and authors.
3. **Занятие**: generate a draft or create your own plan, open it, edit objective,
   purpose, prerequisites, criteria and steps. `[x]` marks work, not mastery.
   Start/pause/resume explicitly. Passing 15 minutes does not complete the lesson.
4. Own results remain verbatim. Local drafts are checkpointed approximately once
   per second while the event loop runs; explicit Save confirms durable storage.
   After a crash, recover a paused session rather than inventing elapsed work.
5. **Память**: add units and independent self-assessments. They do not create
   ratings or scheduler state. A group preserves its members; it does not merge
   or average their FSRS states.
6. If you want a schedule, explicitly enter its first delay in days and desired
   retention. No universal first-check date is imposed. Snooze changes no grade.
7. **Проверка** supports a deliberately limited exact-answer code-check. Choose a
   unit, precommit question and expected answer, then submit your independent
   response or mark unknown/blocked/skip. Revealing an answer marks assistance
   before displaying it. This is not a universal semantic checker. Only mapped,
   code-checked, independent outcomes can update an explicitly enabled schedule.

## Commands for your desktop shortcut editor

- `…/cognitive-exoskeleton context` — open context and explicitly capture.
- `…/cognitive-exoskeleton note` — small own-note window; Ctrl+Enter saves,
  Escape cancels the window (already-confirmed drafts remain local).
- `…/cognitive-exoskeleton subjects` — subjects (Alt+S intent).
- `…/cognitive-exoskeleton session` — planning (Alt+I intent).
- `…/cognitive-exoskeleton memory` — memory units and optional scheduling.

Choose bindings through your desktop's shortcut settings. Existing legacy
Alt-key bindings are not overwritten. Wayland does not permit a portable
application to promise unrestricted global hotkeys; actual bindings and focus
behavior require desktop acceptance in the supported environment.

## Honest limits

MRT is off by default. The author approved [protocol v1](decisions/mrt-protocol-v1.md):
after a verified error, optional diagnostic notification / silence (50/50),
15-minute minimum spacing, at most 3 assignments per UTC day, a different
independent check of the same unit within 24 hours as outcome. Enable it explicitly
in **Эксперимент**, with separate consent outside a session. Clicks do not change
FSRS. Reports are descriptive, not proof of benefit. `mrt-report` explicitly
exports the auditable JSON. `diagnostics` exports counts only, no raw text or keys.

Historical SQLite archives can be inspected without running the old daemon:
`cognitive-exoskeleton archive /path/to/archive.sqlite3` lists tables; add a table
name to explicitly preview its rows as JSON. This is read-only/immutable; live
WAL databases are refused, not silently truncated to an incomplete snapshot.
The preview is bounded (100 rows, 64 KiB per text cell; truncation is labelled).
It may contain personal text, so run it only when you intend to view/export that
archive. This does not import or change the old database.
Windows desktop tests, focus/hotkey acceptance, packaging installation tests,
resource baselines and the remaining registry rows still gate the full release.
