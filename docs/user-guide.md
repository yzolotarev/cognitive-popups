# Cognitive Popups v1 — working guide

This is an implementation in progress, not a signed-off v1 release. Windows
desktop acceptance and the real desktop hotkey/capture checks are still open.

## Start and install

Running the program without arguments prints this guide's command list and
does not open a window. The five actions are separate windows that appear on
demand; the control panel (`cognitive-popups hub`, or the installed menu entry
«Cognitive Popups — управление») is a separate surface for subjects, sessions,
memory, the experiment and settings, not their menu and not the entry point.
`--help` is a non-GUI diagnostic and works without a desktop.

Linux release packages include `install.sh`: `sh install.sh` installs the
binary into `~/.local/bin` (override the directory with `XDG_BIN_HOME`), the
launcher into the applications menu, and the opt-in keybind examples into
`~/.local/share/cognitive-popups/`. The binary has to be reachable **by name**,
because the hotkeys run `cognitive-popups <action>`; `cognitive-popups
--version` prints the version of the installed file, which is how you check that
a hotkey starts the build you just installed. There is nothing to autostart,
because every action is a one-shot command bound to a hotkey. Windows packages
include `install.ps1` (user-local Start menu shortcut; optional `-Autostart`).
Neither installer edits your compositor or hotkey configuration.

Hyprland 0.56 with a Lua config (Ryoku) reads `hyprland.lua`; the classic
`hyprland.conf` example carries the same five binds. Both are examples only:
remove old binds for the same keys first, then add the five lines and reload
with `hyprctl reload`. Alt+I opens the session window, not the control panel.

Linux needs the runtime libraries used by Slint (Wayland/X11, EGL, fontconfig),
`wl-clipboard` on Wayland or `xclip` on X11, and an unlocked Secret Service for
API keys. There is no insecure key-file fallback. Windows uses Credential
Manager; automatic selection capture and global hotkey acceptance are not yet
verified on a Windows desktop.

Data lives under the OS state directory `cognitive-exoskeleton-v1` (Linux:
usually `~/.local/state/cognitive-exoskeleton-v1`). The internal directory name
is historical; the product name is Cognitive Popups. Earlier databases under
`cognitive-exoskeleton` are left untouched and never imported automatically.
The historical application is never launched or modified by this runtime.

## The five actions

| Action | Command | Meaning |
|---|---|---|
| Alt+E | `cognitive-popups comment` | comment on the current fragment |
| Alt+W | `cognitive-popups four-words` | Four Words: four layered semantic anchors |
| Alt+T | `cognitive-popups task` | exercise → your own attempt → feedback |
| Ctrl+Alt+P | `cognitive-popups prediction` | your hypothesis **before** comparison |
| Alt+I | `cognitive-popups session` | own session window: timer, subject, goal, plan, start/pause |

`cognitive-popups note` is the shortcut-free version of the comment window.
`--manual-source` reads the material from stdin instead of capturing a
selection; it is explicit and is used by the hub's "open tool" buttons so the
same text is shown without a second hidden capture.

Wayland does not let an ordinary application promise unrestricted global
hotkeys. Bind the commands in your desktop's shortcut settings, or include the
example `hyprland.conf` shipped next to the installed binary:

```
source = ~/.local/share/cognitive-popups/hyprland.conf
```

It is **not** sourced automatically. Remove or move any legacy Alt+E/W/T/I
bindings first; the old daemons are still running on some systems and must not
receive the same key by accident.

## Settings and privacy

In **Настройки**, choose the provider slot (default / context / plan / Four
Words / task / prediction); enter the full OpenAI-compatible
`https://…/v1/chat/completions` endpoint, model ID and your API key. HTTP is
allowed only for loopback. Keys go to the system credential store, never to
SQLite or diagnostics. A blank key keeps the existing key for that exact
endpoint; changing the endpoint does not reuse another endpoint's key.
Redirects are rejected and failed requests are not retried silently.

A request is cut off after 60 seconds and the window keeps your input; press the
button again to retry. Use a flash-class model: slow reasoning models regularly
exceed the limit, and a popup that waits two minutes is not a popup. The
provider route (endpoint and model) is printed in each window before you press
anything, so you always know where the fragment would go.

Network requests happen only after an explicit button. Context sends the
visible material and your thought. Planning sends the selected subject,
material, that subject's previous plans/self-reports and code-check evidence.
Four Words and task/prediction send the shown fragment and, for comparison, your
frozen own answer. This is personal data. No screen or audio recording and no
clipboard watcher runs. Local raw texts are retained; no unapproved TTL or
auto-deletion policy has been introduced.

## Working loop

1. **Предметы**: create a subject with a long-term goal and context. Optional
   preferences are integer weights in a JSON object over the mechanisms
   `four_words`, `explain`, `example`, `exercise`. Unknown names are rejected,
   `0` excludes a mechanism, and a zero sum means "choose explicitly" — no
   mechanism is substituted silently. Automatic reweighting by lessons is not
   enabled without the outstanding policy decision.
2. **Контекст**: capture a fragment explicitly or paste/type material. The
   label states the real provenance: a selection, a manual fragment, or the
   clipboard (whose age and window are unverified). Your thought and AI output
   have separate fields and authors.
3. **Занятие (Alt+I, отдельное окно)**: the window opens with the timer, the
   session picker and the plan underneath it. Generate a draft with the model
   (it opens immediately under the timer as a draft) or write your own plan;
   edit objective, purpose, prerequisites, criteria and steps. `[x]` marks
   work, not mastery. Start/pause/resume only by an explicit button. Passing
   15 minutes does not complete a lesson and closing the window pauses nothing
   silently — the stored state stays, and an Active session is recovered to
   paused on the next open. In the task window the success criterion stays
   hidden until your own answer is frozen, so it cannot hand you the solution.
4. Own results stay verbatim. Drafts checkpoint locally while the event loop
   runs; explicit Save confirms durable storage. After a crash, recover a
   paused session instead of inventing elapsed time.
5. **Память**: add units and independent self-assessments. They do not create
   ratings or scheduler state. A group preserves its members; it does not merge
   or average FSRS states.
6. If you want a schedule, enter its first delay in days and desired retention
   explicitly. No universal first-check date is imposed. Snooze changes no grade.
7. **Проверка** is a deliberately limited exact-answer code-check. Choose a
   unit, precommit the question and expected answer, then answer independently
   or mark unknown/blocked/skip. Revealing the answer records assistance before
   display. This is not a universal semantic checker. Only mapped, code-checked,
   independent outcomes can update an explicitly enabled schedule.
8. **Опыт** is the opt-in MRT experiment. It is off by default; enabling it
   requires separate consent for in-session and outside-session delivery. The
   report is descriptive (Wilson intervals, missing outcomes), never a claim
   that the product improves learning.

## Four Words

Four Words is four meaning supports, not a four-word summary. Each anchor has
three layers: a simple everyday anchor → the precise term from the material →
its role or relation in this source. Click an anchor or press `1`–`4` to move
through the layers; `Escape` closes the window. The source stays primary and the
tool is assistance, not proof that you understood it.

## Task and prediction windows

`task` creates one exercise from the shown fragment; the question and criterion
are stored before you answer. Your answer is frozen and saved verbatim before
any comparison. Model feedback is labelled as LLM feedback, not objective
verification, and never writes a FSRS grade. `prediction` fixes your hypothesis
before the model sees it, then compares. Closing without answering creates no
attempt; asking for a hint is recorded as assistance.

## Precise, honest limits

- The exact-answer check is not a semantic grader and cannot prove mastery.
- Model output is never treated as a user attempt or as a FSRS review.
- Windows desktop behavior, global hotkeys and real selection capture from a
  browser or PDF are not yet verified.
- On Linux/Hyprland the windows were driven by pointer and keyboard, focus came
  back to the previous window after closing, and notes and subjects were saved
  to the workspace; that evidence is Linux-specific and does not cover Windows.
