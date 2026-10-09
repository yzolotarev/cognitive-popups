# cognitive-popups

![cognitive-popups](assets/cover.jpg)

A small Linux desktop assistant for turning selected text into compact cognitive
prompts. It runs as a Hyprland-friendly GTK utility layer and keeps the current
reading buffer separate from the GUI.

## Naming

The product — and this repository — is **Cognitive Popups**. The idea was developed
under the working name **cognitive-exoskeleton**, so older notes, commit messages
and local folders may still use that word. Both names refer to the same project.

Nothing depends on the folder name. The launchers locate their own checkout, and
`install-user.sh` renders the absolute path into the systemd units, so the clone
directory can be called anything; rerun the installer if you move it later.

## What it does

- extracts four concise cues from the primary selection and shows them as an
  orbital cluster that the reader unfolds one word at a time, so they drill into a
  term instead of being handed all four at once;
- runs a series of three views on the same passage, with a pause for the reader's
  own mind map between them and one closing thesis at the end;
- runs an explicit 15-minute reading step that the reader starts themselves: one
  goal per session (skippable), a passive countdown ring, and at the end a single
  "did you make the step?" window with one line for what was taken away;
- keeps cues in a session buffer and lets the reader check one written hypothesis
  against the source;
- runs a Feynman-style understanding check that names a gap rather than scoring;
- shows one concrete example, compresses a passage, explains an unknown term, or
  answers a free-form question about the material;
- generates practice tasks with a separate solution, checks that a task is
  answerable before showing it, and can revisit archived material for a new task;
- keeps notes about a misreading, anchored to the text they were about;
- builds a "reader's universe" in the background: the reader's own notes,
  hypotheses and explanations are kept word for word, and the model only names
  the concepts in them and the links between them;
- prints one timeline of a study day from all local stores;
- archives cleared sessions locally;
- records every window and click in SQLite, so a reading session can be replayed;
- supports local prompt overrides and optional, private sound cues (no audio is
  bundled).

Background work only uses attention nobody is using: it waits while a step is
running, a window is open or a key was pressed in the last few seconds, and sends
one small request at a time.

Frozen for now, code kept: the "another view" action (`Alt+R`), the side panel
(`Alt+H`) and most sound cues.

The reader drives. No window opens, no mode switches and no check is proposed unless
a hotkey, a click or a command asks for it, and nothing here measures how well
the reading went: a verdict from the model is a stimulus for thinking, not a grade.

The project talks to an OpenAI-compatible local endpoint. The default endpoint is
`http://127.0.0.1:8081/v1/chat/completions`; this repository does not contain API
keys, cookies, or any other credentials.

## Status

This is a working desktop prototype rather than a finished product. The models,
prompt contracts, service flow, ledgers and the GTK surfaces are covered by the test
suite, while the exact desktop integration depends on the local Linux environment.

## Requirements

- Linux with Hyprland or a compatible Wayland/X11 desktop;
- Python 3.10 or newer;
- GTK 3 and PyGObject;
- `wl-paste`/`wl-copy` or `xclip` for selection and clipboard access;
- `hyprctl` for cursor positioning under Hyprland;
- a local OpenAI-compatible model bridge, such as the configured Gemini web2api
  service.

On Debian/Ubuntu, the desktop dependencies are typically available as:

```bash
sudo apt install python3-gi gir1.2-gtk-3.0 wl-clipboard xclip
```

## Install

Clone the repository wherever you like:

```bash
git clone https://github.com/yzolotarev/cognitive-popups.git \
  ~/projects/cognitive-popups
cd ~/projects/cognitive-popups
```

The clone directory name does not matter. If you keep the checkout under
`~/projects/cognitive-popups`, the bundled Hyprland bindings find it without any
configuration; otherwise set `COGNITIVE_PROJECT` to the checkout you use.

Run the tests:

```bash
python3 scripts/run-tests.py     # works where pytest is not installed
python3 -m pytest                # the same suite, if pytest is available
```

Install and start the user service:

```bash
./scripts/install-user.sh
```

Merge `config/hypr-v2.lua` into the Hyprland user configuration. The default
bindings are:

| Key | Action |
|---|---|
| `Alt+W` | four cues from the primary selection |
| `Ctrl+Alt+W` | three views in a row with a mind-map pause, then one thesis |
| `Alt+F` | Feynman check for the current buffer |
| `Alt+E` | note about a misreading, anchored to the selection |
| `Alt+C` | explain terms, or ask a free-form question |
| `Ctrl+Q` | compress the selection to its gist |
| `Alt+I` | start a 15-minute reading step (type another number first for a different length) |
| `Alt+G` | one concrete example (`Alt+Shift+G` adds your own request) |
| `Alt+T` | generate a practice task; `Alt+Shift+T` revisits archived material; `Alt+Y` attempts the last task |
| `Alt+K` | the shortcut reference: what each key does |

The installer also enables `cognitive-universe.timer`, which runs the background
concept naming. Two read-only helpers show what was stored:

```bash
./scripts/cognitive-universe.sh query "some text"   # which of your past thoughts this text touches
./scripts/cognitive-timeline.sh                     # today's study timeline
```

`V2.md` lists every launcher action, including the ones without a default key.

Check the service with:

```bash
systemctl --user status cognitive-popups.service
```

## Configuration

Environment variables can override the defaults:

- `COGNITIVE_API_URL` — chat-completions endpoint;
- `COGNITIVE_MODEL` — model name;
- `COGNITIVE_STATE_DIR` — local history, PID, and SQLite directory;
- `COGNITIVE_POPUP_HELPER` — path to the GTK input helper;
- `COGNITIVE_POPUP_PYTHON` — interpreter used for the helper.
- `COGNITIVE_EVENT_DB` — where the interaction log is written;
- `COGNITIVE_NOTES_DB` — where error notes are written;
- `COGNITIVE_EVENT_DISABLE=1` — turn the interaction log off.

Prompt overrides are stored at
`~/.config/cognitive-popups/prompts.json`. The helper script can list, edit, or
reset them:

```bash
./scripts/cognitive-prompts.py --list
./scripts/cognitive-prompts.py four_words
./scripts/cognitive-prompts.py --reset-all
```

## Development

Run the application directly from the checkout:

```bash
PYTHONPATH=src python3 -m cognitive_popups
```

Run the test suite:

```bash
python3 scripts/run-tests.py     # works where pytest is not installed
python3 -m pytest                # the same suite, if pytest is available
```

The `V2.md` document describes the current architecture and signal flow.

## Local data

Two SQLite files live in `COGNITIVE_STATE_DIR` (default
`~/.local/state/cognitive-popups/`). They are deliberately separate: telemetry
and the reader's own words have different lifetimes and different tolerance for
failure.

```bash
python3 -m cognitive_popups.event_log --tail 40        # windows, clicks, layers
python3 -m cognitive_popups.event_log --prune-days 30  # trim telemetry only
python3 -m cognitive_popups.notes --open               # open error notes
python3 -m cognitive_popups.notes --repeats            # one anchor, N times = a real hole
python3 -m cognitive_popups.notes --by-kind            # group, to decide what to study
python3 -m cognitive_popups.notes --close 7 -m "what I did"
```

The event log never breaks a popup: an unavailable database loses events. The note
store does the opposite and raises, because losing the reader's own words is worse
than losing an event.

## Privacy and security

The application sends selected text to the configured local API endpoint. Review
that endpoint before use. Keep credentials in the local model bridge or an
ignored environment file; never commit them to this repository.

## License

MIT. See [LICENSE](LICENSE).
