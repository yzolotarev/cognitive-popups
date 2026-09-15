# cognitive-popups

A small Linux desktop assistant for turning selected text into compact cognitive
prompts. It runs as a Hyprland-friendly GTK utility layer and keeps the current
reading buffer separate from the GUI.

## What it does

- extracts four concise cues from the primary selection;
- reveals those cues one word at a time, so the reader drills into a term instead
  of being handed all four at once;
- keeps those cues in a local session buffer;
- runs a Feynman-style understanding check;
- shows the result in compact desktop popups;
- records every window and click in SQLite, so a reading session can be replayed;
- keeps error notes: what the reader thought a phrase meant, and what to do about it;
- archives cleared sessions locally;
- supports local prompt overrides.

The project talks to an OpenAI-compatible local endpoint. The default endpoint is
`http://127.0.0.1:8081/v1/chat/completions`; this repository does not contain API
keys, cookies, or any other credentials.

## Status

This is an early desktop prototype. The core models, prompt contracts, service
flow, history, and GTK integration are covered by tests, while the exact desktop
integration depends on the local Linux environment.

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

Clone the repository at the path expected by the example systemd unit:

```bash
git clone https://github.com/yzolotarev/cognitive-popups.git \
  ~/projects/cognitive-popups
cd ~/projects/cognitive-popups
```

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

- `Alt+W` — extract four cues from the primary selection;
- `Alt+F` — start a Feynman check;
- `Alt+E` — record an error note about the current selection;
The compact panel can be opened with `./scripts/cognitive-popups-signal.sh menu`.

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
