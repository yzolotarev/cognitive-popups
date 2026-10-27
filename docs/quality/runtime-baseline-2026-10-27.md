# Local runtime baseline — not release acceptance

Environment: Arch Linux, Hyprland 0.56, two outputs (1.0 and 1.25 scale),
Rust 1.98.0, Slint 1.18.1; x86_64 Linux release build. Measured while developing
the full-v1 implementation branch. These are a baseline, not approved SLA values.

## Executed checks

- Release build (`cargo build --locked --workspace --release`): exit 0.
- `credential-probe`: synthetic write/read/delete in the actual OS protected
  credential store, exit 0. No existing secret read/enumeration, no real API call.
- Smoke window: exit 0 after 5.10 seconds (five-second programmed lifetime).
- Hub launch with isolated XDG state: mapped native toplevel, submitted buffer
  and surface commits, no panic in the observed three-second lifetime. This is
  renderer/process evidence, not keyboard/focus or composited-visibility acceptance.
- User-local Linux install script in a temporary HOME: exit 0; installed binary
  `--help` exit 0; desktop launcher exists; autostart absent by default. Does not
  prove installation on a clean distribution or Windows.
- First-launch diagnostics in the isolated HOME: empty objects, screen recording
  false, interventions false; no old data imported or real key present.

## Measurements

| Trial | Process start to first Wayland buffer attach (ms) | RSS at 2 s (KiB) | CPU over following 1 s (% of one core) |
|---|---:|---:|---:|
| 1 | 217.61 | 126936 | 0.0 |
| 2 | 207.51 | 126772 | 0.0 |
| 3 | 205.77 | 126348 | 0.0 |

Warm executable `--help` subprocess, 10 observations, milliseconds sorted:
`1.65, 1.70, 1.77, 1.85, 1.88, 1.94, 1.97, 2.04, 2.43, 2.87`.
This is a CLI startup measurement, not GUI or inference latency. Three GUI trials
are insufficient for a robust p95 claim. CPU sampling resolution can hide brief
work; 0.0 does not mean no CPU is ever used. No live-provider latency measured.

## Screen/focus restriction

Screencopy in this session shows wallpaper and a greeter-like interface with an
input field, while `loginctl` reports `Active=yes`, `State=active`,
`LockedHint=no`; normal client windows are not visible in that capture.
The displayed input was not confirmed as the keyboard-focus target. Therefore
the previous hypothesis "grim is broken" is not an established diagnosis, nor is
a locked-session claim established. No bypass, credential/config storage or
desktop reconfiguration was performed to force the check. Composited visibility,
keyboard focus and return-to-source remain **NOT_VERIFIED**.

Raw temporary probe files were under `/tmp/opencode/`; this self-contained
summary is the durable record. It is not E5/E6 evidence and does not close P1/P7.
