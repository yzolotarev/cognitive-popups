# Cognitive Popups v1 — working guide

This is an implementation in progress, not a signed-off v1 release. Windows
desktop acceptance and the real desktop hotkey/capture checks are still open.

## Start and install

Run `cognitive-popups hub` (or launch without arguments). `--help` is a
non-GUI diagnostic and works without a desktop.

Linux release packages include `install.sh`: `sh install.sh` installs a
user-local binary and launcher; `--autostart` is opt-in. Windows packages
include `install.ps1` (user-local Start menu shortcut; optional `-Autostart`).
Neither installer edits your compositor or hotkey configuration.

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
| Alt+I | `cognitive-popups hub` | subject, goal, plan and session |

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
2. **Контекст**: capture a fragment explicitly or paste/type material.
   Clipboard fallback is labelled unverified/stale-possible; confirm the
   displayed text yourself. Your thought and AI output have separate fields and
   authors.
3. **Занятие**: generate a draft or write your own plan; edit objective,
   purpose, prerequisites, criteria and steps. `[x]` marks work, not mastery.
   Start/pause/resume explicitly. Passing 15 minutes does not complete a lesson.
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
