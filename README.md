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
- optionally shows the same cues in one "sand" window instead (off by default):
  grains lift off the selection and settle into the four words at its corners, in
  a colour opposite to the page, and each word unfolds its explanation outward;
- runs a series of three views on the same passage, with a pause for the reader's
  own mind map between them and one closing thesis at the end;
- runs an explicit 15-minute reading step that the reader starts themselves: one
  goal per session (skippable), a passive countdown ring, and at the end a single
  "did you make the step?" window with one line for what was taken away;
- keeps cues in a session buffer and lets the reader check one written hypothesis
  against the source: the answer stays on screen until closed and says what holds,
  where to look (a quote from the text) and the nodes, never the missing piece;
- brings a missed hypothesis back after 2, 5, 12 and 30 days, without the text,
  at most once a day; "confirmed later" only counts on another day;
- ends a step with one optional self-check click (simplified, grouped, related to
  the bigger picture, recalled without the text);
- shows a weekly overview of the reader's own misses, repeated stumbles and
  returns, with no scores, streaks or minutes (`Alt+Shift+K`, `cog stats`);
- after the first four words of a session, sends the words to the corners of a
  sheet for under a second: the links are for the reader to draw by hand;
- saves each closed session as an SVG card (misses bright) in `<state>/sky/`;
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
- supports local prompt overrides (an override written for an older built-in
  prompt is noticed and ignored) and optional, private sound cues (no audio is
  bundled);
- when the model is down, says so in one line and offers up to four words taken
  from the text itself, marked as a draft made without AI.

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

Version 1.0: meant to keep working for months without updates. The models,
prompt contracts, service flow, ledgers and the GTK surfaces are covered by the test
suite, while the exact desktop integration depends on the local Linux environment.
`cog doctor` checks that integration on your machine.

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
| `Ctrl+Alt+P` | your hypothesis, checked against the text |
| `Alt+E` | note about a misreading, anchored to the selection |
| `Alt+C` | explain terms, or ask a free-form question |
| `Alt+I` | start a 15-minute reading step (type another number first for a different length) |
| `Alt+K` | the shortcut reference: what each key does |
| `Alt+Shift+K` | every action: weekly overview, compress to the gist, and the rest |
| `Ctrl+Alt+Shift+P` | saved answers to hypotheses |
| `Ctrl+Alt+W` | three views in a row with a mind-map pause, then one thesis |
| `Alt+F` | Feynman check for the current buffer |
| `Alt+G` | one concrete example (`Alt+Shift+G` adds your own request) |
| `Alt+T` | generate a practice task; `Alt+Shift+T` revisits archived material; `Alt+Y` attempts the last task |

The first six are the everyday set. So that the service sees your display, add
this line to the Hyprland configuration as well:

```text
exec-once = systemctl --user import-environment DISPLAY WAYLAND_DISPLAY
```

The installer also enables `cognitive-universe.timer`, which runs the background
concept naming. Two read-only helpers show what was stored:

```bash
./scripts/cognitive-universe.sh query "some text"   # which of your past thoughts this text touches
./scripts/cognitive-timeline.sh                     # today's study timeline
```

`V2.md` lists every launcher action, including the ones without a default key.

Check the installation and look at your data from the terminal:

```bash
./scripts/cog doctor      # read-only self-check: GTK, clipboard, service, binds, model, databases
./scripts/cog stats       # the weekly overview (--days N, --all, --json)
./scripts/cog maintain    # trim telemetry and old backups (also runs weekly by timer)
./scripts/cog notes       # your error notes
./scripts/cog tail 40     # the last interaction events
```

The service starts without the model bridge; everything that needs no model
(notes, steps, returns, the overview) works offline.

## Configuration

Environment variables can override the defaults:

- `COGNITIVE_API_URL` — chat-completions endpoint;
- `COGNITIVE_MODEL` — model name;
- `COGNITIVE_API_KEY` — sent as a Bearer token, if your endpoint needs one;
- `COGNITIVE_TIMEOUT` — seconds to wait for the model;
- `COGNITIVE_STATE_DIR` — local history, PID, and SQLite directory (default
  `$XDG_STATE_HOME/cognitive-popups`, that is `~/.local/state/cognitive-popups`);
- `COGNITIVE_POPUP_HELPER` — path to the GTK input helper;
- `COGNITIVE_POPUP_PYTHON` — interpreter used for the helper.
- `COGNITIVE_EVENT_DB` — where the interaction log is written;
- `COGNITIVE_NOTES_DB` — where error notes are written;
- `COGNITIVE_EVENT_DISABLE=1` — turn the interaction log off.

The sand window has its own switches:

```bash
./scripts/cognitive-sand.sh on       # use the sand window for four cues
./scripts/cognitive-sand.sh off      # back to the orbital cluster
./scripts/cognitive-sand.sh grains   # words made of sand only (trial mode)
./scripts/cognitive-sand.sh text     # sand settles into real text
./scripts/cognitive-sand.sh unmute   # sand sound on (off by default)
./scripts/cognitive-sand.sh mute     # sand sound off
./scripts/cognitive-sand.sh reduced  # less motion in every animated window
```

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

Four SQLite files live in `COGNITIVE_STATE_DIR` (default
`~/.local/state/cognitive-popups/`): `records` (sessions, fragments, hypotheses,
steps, returns), `notes`, `events` and `universe`. Telemetry and the reader's own
words have different lifetimes and different tolerance for failure: `cog maintain`
trims telemetry and keeps the three newest backups, and never deletes notes,
hypotheses or the step journal. A ledger schema is never lowered by an older
checkout. On upgrade, an existing `<checkout>/var` with data is linked from the
state directory; nothing is moved.

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
