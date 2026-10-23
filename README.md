# Ascuta Desktop

Desktop companion for **Ascuta** (Android). The phone app transcribes speech
**on-device**; Ascuta Desktop receives the text from the phone and **types it on
the computer** as if it had been written on the keyboard.

Audio transcription and models stay on the phone: only text travels to the
computer, over the local network. No cloud, no account.

> **Status: M0 (scaffold).** The *phone bridge* works and can be tested.
> Local transcription on the desktop (sherpa-onnx models, push-to-talk hotkey,
> HUD) is in development — see [`PLAN.md`](PLAN.md).

---

## What it does today

- **Phone bridge** — receives transcripts from the Ascuta app over Wi-Fi and
  types them into the focused field. Pairing via **QR code**.
- **`--pair`** — headless mode: prints the QR in the terminal and listens,
  without tray or UI. The simplest way to try it.
- **`--check`** — self-test without display, audio or network.
- Tray, global hotkey (`Ctrl+Win`, hold to talk) and HUD: scaffold in place.

---

## Requirements (Linux)

- Linux **x86_64**.
- Typing works in **X11** and **Wayland** sessions. On Wayland Ascuta uses the
  XDG RemoteDesktop portal; if it is unavailable, the text is left in the
  clipboard for manual paste.
- Recommended for the clipboard: `xclip` (X11) or `wl-clipboard` (Wayland).
- The prebuilt binary needs a few system libraries (`libc`, ALSA, `bzip2`);
  X11/Wayland are loaded at runtime, so no development packages are required.
  If something is missing, `install.sh` takes care of it.

---

## Quick install (Linux)

```sh
curl -fsSL https://github.com/emavgl/ascuta-website/releases/download/desktop-latest/install.sh | bash
```

The installer downloads the **prebuilt binary** (no source code), verifies its
SHA-256 checksum, installs any missing system libraries, installs into
`~/.local` (no sudo for the files), adds **Ascuta Desktop** to the menu, runs a
self-test and, if you want, adds you to the `input` group for the global hotkey
(e.g. `Ctrl+Win`). The speech model is downloaded by the app on first launch
(~600 MB).

Prefer downloading the package yourself:

```sh
curl -fLO https://github.com/emavgl/ascuta-website/releases/download/desktop-latest/ascuta-desktop-linux-x86_64.tar.gz
curl -fLO https://github.com/emavgl/ascuta-website/releases/download/desktop-latest/ascuta-desktop-linux-x86_64.tar.gz.sha256
sha256sum -c ascuta-desktop-linux-x86_64.tar.gz.sha256
tar -xzf ascuta-desktop-linux-x86_64.tar.gz
cd ascuta-desktop-v*-linux-x86_64
./install.sh
```

Useful options: `--yes` (no questions), `--from URL` (use another package),
`--uninstall [--purge]`, `--help`.

The package is built with `packaging/build-release.sh` (requires
`libasound2-dev`) and published manually to the public website repository (see
[Publishing a release](#publishing-a-release)). The source repository is
private; end users only get the binary.

---

## macOS and Windows

Same code, three systems. What changes:

| | Linux | macOS | Windows |
|---|---|---|---|
| Package | `.tar.gz` + `install.sh` | `.dmg` with `Ascuta.app` | `.zip` + `install.ps1` |
| Hotkey (default) | `Ctrl+Win` | `Option+Space` | `Ctrl+Win` |
| Tray | StatusNotifier | menu bar | notification area |
| Paste | `Shift+Insert` | `Cmd+V` | `Ctrl+V` |
| Launch at login | `~/.config/autostart` | LaunchAgent | `Run` registry key |

**macOS** — on first launch the system asks for three permissions (System
Settings → Privacy & Security): *Microphone*, *Accessibility* (to paste) and
*Input Monitoring* (to read the hotkey). The app is ad-hoc signed: the first
time, open it with right-click → Open.

**Windows** — no special permissions. `install.ps1` installs for the current
user and creates a Start menu entry. SmartScreen may warn because the executable
is not signed.

The optional Gemma cleanup uses Python: on macOS/Windows you need `uv` or
Python 3 (the *Download* button installs the runtime itself).

Build from source: `cargo build --release --features "capture sherpa"`
(macOS: `packaging/macos/build-app.sh`; Windows: `packaging/windows/build-zip.ps1`).

---

## 1. Downloading the project

End users install the **prebuilt binary** with the one-line command above, or by
downloading `ascuta-desktop-linux-x86_64.tar.gz` and its `.sha256` from the
[public release page](https://github.com/emavgl/ascuta-website/releases/tag/desktop-latest).
No source code is distributed.

The source repository is private. Maintainers building locally use:

```sh
git clone git@github.com:emavgl/ascuta-desktop.git
cd ascuta-desktop

# compile and install to ~/.local (requires Rust and libasound2-dev)
cargo build --release --locked --features "capture sherpa"
./packaging/build-release.sh
./dist/ascuta-desktop-v*-linux-x86_64/install.sh
```

### Publishing a release

Run this from a machine authenticated with `gh` (no extra token needed):

```sh
packaging/build-release.sh v0.2.2

gh release view desktop-latest -R emavgl/ascuta-website >/dev/null 2>&1 \
  || gh release create desktop-latest -R emavgl/ascuta-website \
       --title "Ascuta Desktop (latest)" \
       --notes "Latest Ascuta Desktop build. Assets are replaced on every release."

gh release upload desktop-latest -R emavgl/ascuta-website --clobber \
  dist/ascuta-desktop-linux-x86_64.tar.gz \
  dist/ascuta-desktop-linux-x86_64.tar.gz.sha256 \
  dist/install.sh
```

`packaging/build-release.sh` also leaves the versioned package in `dist/`; the
stable-named files above are what `install.sh` downloads.

---

## 2. Installing on Linux

With the prebuilt package, the quickest path is:

```sh
./install.sh
```

The script installs the binary, the bundled libraries, the desktop entry and the
`PATH` entry. If you prefer to do it by hand:

```sh
mkdir -p ~/.local/bin
install -m755 ascuta-desktop-*/ascuta-desktop ~/.local/bin/ascuta-desktop
```

Make sure `~/.local/bin` is in `PATH` (it usually is):

```sh
echo "$PATH" | tr ':' '\n' | grep -q "$HOME/.local/bin" \
  || echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
```

- **Normal start:** `ascuta-desktop`
- **Settings file:** `~/.config/ascuta/settings.json`
  (override with the `ASCUTA_CONFIG_FILE` variable).

---

## 3. Trying it

### 3.1 Self-test (no display required)

```sh
ascuta-desktop --check
```

It must end with:

```
ascuta --check: OK (press/release/paste-hello/phone-bridge/settings-roundtrip)
```

### 3.2 Trying the phone bridge (main use case)

1. **On the computer** start pairing mode:

   ```sh
   ascuta-desktop --pair
   ```

   It prints a **QR code** in the terminal and listens on port `8737`. Keep the
   terminal open; use `Ctrl+C` to stop it.

2. **On the phone** open Ascuta and go to
   **Home → Send to computer** (turn the switch on) → **Pair a computer**, then
   **scan the QR** shown on the computer.

3. **Dictate** with Ascuta (bubble, keyboard or popup). The final text is typed
   on the computer into the focused window.

4. **Test connection** (Home → Send to computer) checks that the computer is
   reachable.

Notes:

- Phone and computer must be on the **same Wi-Fi network**; the firewall must
  allow port `8737`.
- Security is local: bind on the LAN and a token generated in the QR. No data
  leaves the local network.
- You need a recent version of the Ascuta app that includes the bridge (repo
  `ascuta`).

### 3.3 Normal mode (desktop with a display)

```sh
ascuta-desktop
```

Starts the tray, the global hotkey (`Ctrl+Win`, hold to talk) and the HUD; the
bridge starts only if `bridge_enabled` is `true` in `settings.json`. If the tray
or hotkey is unavailable (e.g. Wayland, GNOME without the extension), the
program warns and keeps running.

---

## 4. Troubleshooting

| Symptom | Cause / Solution |
|---|---|
| Text is not typed, `no key backend` appears | The session has no key backend (e.g. restricted Wayland): the text stays in the clipboard, paste it manually. |
| `address already in use` at startup | Port `8737` is busy: change `bridge_port` in `~/.config/ascuta/settings.json` and restart (the QR will use the new port too). |
| No tray icon on GNOME | Install the *AppIndicator* extension or use `--pair`. |
| The QR is hard to read | The QR uses Unicode block characters: use a terminal with a monospace font. |
| Verifying the download | `sha256sum -c ascuta-desktop-*.tar.gz.sha256` |

---

## 5. Development

```sh
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --check   # headless gate
```

Architecture, models and roadmap: [`PLAN.md`](PLAN.md).

The GitHub Action
[`.github/workflows/release.yml`](.github/workflows/release.yml) builds the Linux,
macOS and Windows packages when a tag is pushed (`git tag v0.1.0 && git push
origin v0.1.0`) and attaches them to the private repository's release. Publishing
the Linux binary to the public website repository is done manually — see
[Publishing a release](#publishing-a-release).

---

## License

MIT.
