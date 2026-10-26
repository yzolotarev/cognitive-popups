# Cognitive Exoskeleton

New Rust implementation for Windows and Linux. This repository currently contains
only a buildable command-line scaffold, not a working desktop application.
No GUI framework has been selected. Legacy Python code remains in Git history.

## Development

Install [rustup](https://rustup.rs/), then run:

```sh
cargo run --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo build --locked --release
```

The toolchain and lint components are pinned in `rust-toolchain.toml`.
Commit `Cargo.lock` when changing dependencies.

## GitHub Actions

Every pull request and push to `main` runs formatting, dependency auditing,
Clippy, tests, release builds and executable smoke tests. Builds run natively on
Windows Server 2022 and Ubuntu 24.04 (x86-64). Download the resulting executables
from the run's **Artifacts** section; these are development scaffolds, not installers.
Linux executables are built against Ubuntu 24.04 and are not promised to run on
older distributions. Artifact downloads may require restoring executable permission
on Linux (`chmod +x cognitive-exoskeleton`).

CI uses no LLM keys, personal data or repository secrets. Hosted runners do not
validate desktop interactions, global hotkeys, selection capture, Wayland/X11
compatibility or Windows permissions. Product invariant tests will be added with
the corresponding implementations; the initial test only checks scaffold identity.

Repository administrators should require these checks in a `main` branch rule:

- `Rust formatting`
- `Build and test (Linux)`
- `Build and test (Windows)`
- `Dependency audit`

Installer packaging, code signing and GitHub Releases are not configured yet.
