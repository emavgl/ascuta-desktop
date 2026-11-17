# Security policy

Ascuta Desktop runs fully offline: there is no account, no telemetry, and no
audio or transcript ever leaves the machine.

## Reporting a vulnerability

Please **do not open a public issue** for a security problem. Use GitHub's
private [Report a vulnerability](https://github.com/emavgl/ascuta-desktop/security/advisories/new)
form, or email **emavgl@gmail.com** with:

- the version (`ascuta-desktop --version` or the release tag),
- your OS and session type (X11 / Wayland / Windows / macOS),
- steps to reproduce, and the impact you believe it has.

You will get an acknowledgement as soon as possible. Please allow time for a
fix before disclosing publicly.

## Threat model and known limits

- **Phone bridge.** The bridge listens on the LAN and requires a 256-bit token
  generated at pairing time, sent as `Authorization: Bearer …`. Requests use
  plain HTTP on the local network: anyone who can sniff the LAN *and* has the
  token can inject text. Bind the bridge only on trusted networks, or leave it
  disabled (`bridge_enabled: false`).
- **Settings at rest.** `settings.json` contains the bridge token and is stored
  with the normal per-user permissions of your OS account. It is not encrypted.
- **Input injection.** To type into other applications, Ascuta needs permission
  to send key events (Linux `input` group, macOS Accessibility/Input Monitoring,
  Windows `SendInput`). The same permission could be abused by any local
  process running as your user; Ascuta never sends events that you did not
  dictate.
- **Model downloads.** Models come from the sherpa-onnx release buckets over
  HTTPS. When a model entry has a known SHA-256 it is verified after download.
- **Bundled cleanup model (optional).** Gemma cleanup runs through a local
  Python sidecar (LiteRT-LM) and never contacts the network after installation.

## Scope

Reports about the Android app, the website, or third-party models should go to
their respective repositories. Reports about the code in this repository,
its build scripts, or the release artifacts are in scope.
