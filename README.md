# Cognitive Popups

Cross-platform desktop application for supporting your own thinking and learning while
you work in a browser, a PDF, an editor or an external trainer. The product contract is
the [versioned specification](docs/spec/cognitive_exoskeleton_v0_2_full_spec.md);
decisions in it are not replaced by this README.

Current state: **full v1 implementation in progress; not release-accepted**.
Slint 1.18 desktop UI includes provider settings, context/own notes, subjects,
editable learning sessions with pause/recovery, memory/self-assessments and
explicit opt-in FSRS with frozen exact-answer checks. See the
[working user guide](docs/user-guide.md) for behavior and honest limits.
The author approved MRT protocol v1 during implementation; opt-in assignment,
system notifications, silence control and descriptive reports are connected to
stored code-check errors. Real desktop acceptance still gates the full product.
Legacy Python code remains in Git history and is not imported into the new runtime.

## Requirements baseline (P0)

| Artefact | Purpose |
|---|---|
| [`docs/spec/…v0_2_full_spec.md`](docs/spec/cognitive_exoskeleton_v0_2_full_spec.md) | Product specification, version 0.2 (27.10.2026) |
| [`docs/requirements/registry.md`](docs/requirements/registry.md) | Every requirement ID, status, stage, implementation and evidence |
| [`docs/decisions/open-decisions.md`](docs/decisions/open-decisions.md) | OD-01…OD-13 cards: options, consequences, blocked requirements |
| [`docs/requirements/legacy-inventory.md`](docs/requirements/legacy-inventory.md) | Evidence-based KEEP/CHANGE/DROP input for historical features |
| [`docs/quality/acceptance-scenarios.md`](docs/quality/acceptance-scenarios.md) | End-to-end scenarios E2E-01…12 and evidence format |
| [`docs/spikes/platform-matrix.md`](docs/spikes/platform-matrix.md) | Proposed supported environments, platform probes S1–S8 |
| [`docs/decisions/full-product-contract.md`](docs/decisions/full-product-contract.md) | The FULL PRODUCT SKILL adopted as the main contract, with its SHA-256 |
| [`docs/decisions/mrt-protocol-v1.md`](docs/decisions/mrt-protocol-v1.md) | Approved MRT protocol v1: consent, limits, outcome window, reporting |
| [`docs/decisions/rewrite-map.md`](docs/decisions/rewrite-map.md) | Old behaviour → new scenario → acceptance, and what was deliberately not transferred |
| [`docs/quality/desktop-evidence-2026-10-27.md`](docs/quality/desktop-evidence-2026-10-27.md) | What was actually verified on a real Linux desktop, and what was not |

`tests/traceability.rs` fails CI when a requirement ID from the specification is missing
from the registry, appears twice, or changes status without the specification changing.

## Development

Install [rustup](https://rustup.rs/), then run:

```sh
cargo run --locked -p cognitive-popups -- hub
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-targets --all-features
cargo build --locked --workspace --release
```

The toolchain and lint components are pinned in `rust-toolchain.toml`.
Commit `Cargo.lock` when changing dependencies.

## GitHub Actions

Every pull request and push to `main` runs formatting, dependency auditing,
Clippy, tests, release builds and executable smoke tests. Builds run natively on
Windows Server 2022 and Ubuntu 24.04 (x86-64). Download the resulting executables
from the run's **Artifacts** section. Packages include user-local installation scripts;
these are development builds, not signed/accepted releases.
Linux executables are built against Ubuntu 24.04 and are not promised to run on
older distributions. Artifact downloads may require restoring executable permission
on Linux (`chmod +x cognitive-popups`).

CI uses no LLM keys, personal data or repository secrets. Hosted runners do not
validate desktop interactions, global hotkeys, selection capture, Wayland/X11
compatibility or Windows permissions. Those rows close only through the protocol in
`docs/spikes/platform-matrix.md`, not through a green CI run.

Repository administrators must require these checks in the `main` branch rule:

- `Rust formatting`
- `Build and test (Linux)`
- `Build and test (Windows)`
- `Dependency audit`

Code signing and publishing GitHub Releases are not configured yet. Packaging CI
does not replace installation and desktop acceptance on actual supported machines.
