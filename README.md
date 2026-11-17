# Ascuta Desktop

Offline dictation for your computer. Hold a hotkey, speak, release — the text is
typed into whatever app has focus.

Speech recognition runs **on your machine** with [sherpa-onnx][sherpa]
(Parakeet or Whisper). No account, no cloud, no telemetry. The app is also the
desktop companion of [Ascuta for Android][android]: the phone can send finished
transcripts here over Wi-Fi, which gives any computer a microphone.

[sherpa]: https://github.com/k2-fsa/sherpa-onnx
[android]: https://play.google.com/store/apps/details?id=eu.viglianisi.ascuta

## Features

- **Push-to-talk dictation.** Hold a key chord (default `Ctrl+Win`, `Option+Space`
  on macOS), speak, release. Modifier-only chords work on X11, Wayland, Windows
  and macOS.
- **Fully offline STT.** Parakeet TDT v2 (English) or v3 (25 European
  languages) for speed and punctuation; Whisper base / small / large-v3-turbo
  for 99+ languages. CPU only — no GPU or CUDA required.
- **Silero VAD.** Silence is trimmed before decoding, and a recording with no
  speech is discarded instead of pasted.
- **Optional local cleanup.** Gemma 4 E2B through LiteRT-LM fixes punctuation
  and obvious mis-hearings. Off by default, runs locally, needs Python (`uv` or
  `python3-venv`).
- **Injection that fits the platform.** Synthetic key events, or clipboard +
  paste when that is more reliable (Wayland falls back to the XDG RemoteDesktop
  portal, and to the clipboard when no portal is available).
- **Phone bridge.** Pair the Android app with a QR code; every finished
  transcript is typed here. Audio never leaves the phone.
- Tray icon, settings window, launch at login, no runtime dependencies beyond
  the bundled libraries.

## Install

### Linux (x86_64)

One line, verifies the checksum and installs into `~/.local` (no `sudo` for the
files; system libraries are installed with your package manager if missing):

```sh
curl -fsSL https://github.com/emavgl/ascuta-desktop/releases/latest/download/install.sh | bash
```

Or download the tarball yourself:

```sh
base=https://github.com/emavgl/ascuta-desktop/releases/latest/download
curl -fLO "$base/ascuta-desktop-linux-x86_64.tar.gz"
curl -fLO "$base/ascuta-desktop-linux-x86_64.tar.gz.sha256"
sha256sum -c ascuta-desktop-linux-x86_64.tar.gz.sha256
tar -xzf ascuta-desktop-linux-x86_64.tar.gz
cd ascuta-desktop-v*-linux-x86_64 && ./install.sh
```

The global hotkey reads keyboards through `/dev/input`, so the installer offers
to add you to the `input` group (log out and back in afterwards). On Wayland the
HUD uses a layer-shell surface and paste goes through the desktop portal; on
GNOME, install the AppIndicator extension for the tray icon.

### Windows (10/11, x86_64)

Download and run
[`ascuta-desktop-windows-x86_64-setup.exe`](https://github.com/emavgl/ascuta-desktop/releases/latest/download/ascuta-desktop-windows-x86_64-setup.exe).
It installs per-user in `%LOCALAPPDATA%\Ascuta`, needs no administrator rights,
bundles the MSVC runtime and registers an uninstaller. SmartScreen may warn
because the executable is not code-signed.

PowerShell alternative (portable zip + per-user install):

```powershell
irm https://github.com/emavgl/ascuta-desktop/releases/latest/download/install-windows.ps1 | iex
```

### macOS (12+, Apple silicon and Intel)

Download the `.dmg` from the [releases page][releases] (the file name ends in
`-macos-arm64.dmg` or `-macos-x86_64.dmg`), open it and drag **Ascuta** to
*Applications*. The app is ad-hoc signed: the first time, open it with
right-click → **Open**.

macOS asks for three permissions: **Microphone**, **Accessibility** (to paste)
and **Input Monitoring** (to read the hotkey).

[releases]: https://github.com/emavgl/ascuta-desktop/releases

## First run

1. Open **Ascuta** from the tray/menu bar (or run `ascuta-desktop`).
2. Download a model: Parakeet v2 for English (~640 MB), Parakeet v3 for 25
   European languages (~670 MB), or a Whisper model for other languages.
   Downloads are HTTPS and verified when a checksum is known.
3. Pick the hotkey, language and input mode. Close the window.
4. Put the cursor in any text field, hold the hotkey, speak, release.

The first dictation after a reboot loads the model; expect a short delay.

## Phone bridge

Use the phone as the microphone of a computer that has none, or dictate on the
phone and have the text appear on the desktop:

1. On the computer: `ascuta-desktop --pair` prints a QR code and listens on
   port `8737`.
2. On the phone: **Ascuta → Home → Send to computer**, pair, scan the QR.
3. Dictate on the phone. The finished transcript is typed on the computer.

Phone and computer must be on the same network, and the firewall must allow TCP
`8737`. The bridge is off by default; enable it in the settings window (or use
`--pair`) and pair again if the computer's address changes.

## Privacy and security

- Audio is captured, decoded and discarded locally. Transcripts are written only
  to the settings/history files on this machine.
- No analytics, crash reporting or network calls except model downloads.
- The bridge requires a 256-bit bearer token generated at pairing time, but it
  uses plain HTTP on the local network: use it on trusted networks only.
- The app needs permission to send key events in order to type into other apps.

See [`SECURITY.md`](SECURITY.md) for the full threat model and how to report a
vulnerability.

## Build from source

Rust 1.88 or newer. The audio and speech backends are opt-in features so the
default build stays headless.

Linux needs ALSA, XKB and Wayland headers:

```sh
sudo apt install build-essential pkg-config libasound2-dev \
  libxkbcommon-dev libwayland-dev libudev-dev
```

```sh
cargo build --release --features "capture sherpa"
# binary: target/release/ascuta-desktop
```

macOS and Windows need only the Rust toolchain; the sherpa-onnx libraries are
fetched as prebuilt binaries during the build.

## Development

```sh
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --check    # headless self-test: no display, audio or network
```

Useful CLI flags:

| Flag | What it does |
|---|---|
| `--check` | Headless self-test (used in CI) |
| `--version` | Print the version |
| `--pair` | Print the phone-bridge QR and serve, no tray/UI |
| `--gui` | Open the settings window (default on first run) |
| `--transcribe FILE` | Transcribe a WAV file with the configured model |
| `--debug-mic` | Record 4 s from the default input and print levels |

Architecture and design rationale: [`PLAN.md`](PLAN.md).
Contributions: [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Troubleshooting

| Symptom | Cause / solution |
|---|---|
| Text is not typed, `no key backend` in the log | No usable key backend (restricted Wayland): the text stays in the clipboard — paste it manually. |
| Hotkey does nothing on Linux | You are not in the `input` group, or the session predates the change: log out and back in. |
| `address already in use` | Port `8737` is busy: change `bridge_port` in `~/.config/ascuta/settings.json` and restart. |
| No tray icon on GNOME | Install the AppIndicator extension, or use `--pair`. |
| Model download fails | Check free disk space and the network; retry from the model list. |
| macOS does not hear you | Grant Microphone, Accessibility and Input Monitoring in System Settings → Privacy & Security. |

Settings live in `~/.config/ascuta/settings.json` (Linux),
`~/Library/Application Support/ascuta/settings.json` (macOS) and
`%APPDATA%\ascuta\settings.json` (Windows). Override with `ASCUTA_CONFIG_FILE`.

## License

Apache License 2.0 — see [`LICENSE`](LICENSE). Portions are derived from
[Paseo](https://github.com/getpaseo/paseo); see [`NOTICE`](NOTICE).
