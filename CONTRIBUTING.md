# Contributing

Thanks for taking the time to help. Small, focused pull requests are easiest to
review and merge.

## Before you start

- Open an issue for anything larger than a bug fix, so the approach can be
  agreed on before you write code.
- Security issues: see [`SECURITY.md`](SECURITY.md) — do not open a public issue.

## Build and test

The app is a Rust workspace; `src-tauri` is the only crate. The native audio and
speech backends are opt-in features (`capture`, `sherpa`) so the default build
stays headless and testable in CI.

Linux needs the ALSA, XKB and Wayland development headers:

```sh
sudo apt install build-essential pkg-config libasound2-dev \
  libxkbcommon-dev libwayland-dev libudev-dev
```

Build and test:

```sh
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --check                      # headless self-test
cargo build --release --features "capture sherpa"
```

Every pull request must keep the four checks green. They run on
[GitHub Actions](.github/workflows/ci.yml) with no display server, no audio
device and no network, so keep new logic behind pure functions and inject the
OS-facing parts.

## Code guidelines

- One behaviour, one implementation. Put shared logic in the smallest module
  that can hold it, free of windowing and audio types when possible.
- New behaviour needs a unit test that fails without the change.
- Do not add telemetry, analytics, crash reporting, or a network call that is
  not required for model downloads.
- Keep the settings file backwards compatible: new fields need
  `#[serde(default)]`.

## Licensing

Contributions are accepted under the Apache License, Version 2.0 (see
[`LICENSE`](LICENSE)). By opening a pull request you confirm you have the right
to submit the code under that license. There is no CLA.

If your change is derived from another project, say so in the pull request and
add the attribution to [`NOTICE`](NOTICE).
