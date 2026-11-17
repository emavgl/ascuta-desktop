# ascuta-desktop — design notes

> Historical design notes. Most of this plan is implemented; the
> [`README`](README.md) describes what actually ships. The document is kept for
> the architecture rationale and the model/runtime decisions.

Port Paseo transcription code 1:1 into a standalone multi-platform desktop
dictation app (macOS, Windows, Linux), Whisper Flow-like. CPU by default, GPU
optional. Whisper + Parakeet via sherpa-onnx. Basic UI for model + hotkey.

Source reference: `https://github.com/getpaseo/paseo` (Apache-2.0).

## 1. Goal / non-goals

Goal: global-hotkey push-to-talk dictation that pastes polished text into any focused app. Offline-first, zero telemetry, transcripts stay on device.

Non-goals (v1): meeting transcription, diarization, cloud STT, mobile apps, LLM rewrite cloud (only optional local cleanup later).

## 2. Paseo code to port 1:1 (inventory)

| Paseo path | What it does | Port to |
|---|---|---|
| `packages/server/src/server/speech/providers/local/sherpa/model-catalog.ts` | Parakeet `v2-int8` (default EN) + `v3-int8` (25 langs), Kokoro TTS entries, URLs, required files | `crates/models/src/catalog.rs` — same IDs, URLs, `requiredFiles`, defaults. Add Whisper entries (`whisper-base-int8`, `whisper-large-v3-turbo-int8`; `small` only post-v1 pending bench) |
| `packages/server/src/server/speech/providers/local/sherpa/model-downloader.ts` | Download + verify + extract to `modelsDir` | `crates/models/src/downloader.rs` — same logic, `%APPDATA%/ascuta/models` (Win), `~/Library/Application Support/ascuta/models` (mac), `~/.local/share/ascuta/models` (Linux), override `ASCUTA_MODELS_DIR` (= `PASEO_LOCAL_MODELS_DIR` equivalent) |
| `packages/server/src/server/speech/providers/local/sherpa/sherpa-offline-recognizer.ts` | `OfflineRecognizer(nemo_transducer, encoder/decoder/joiner.int8.onnx + tokens.txt, numThreads=2, cpu, greedy_search, 16kHz/80dim)` | `crates/stt/src/recognizer.rs` — same config struct, same `assertFileExists` guards |
| `packages/server/src/server/speech/providers/local/sherpa/sherpa-parakeet-stt.ts` + `sherpa-parakeet-realtime-session.ts` | Buffer PCM, `acceptWaveform`, `decode`, `getResult`, commit every ~15s, assemble segments | `crates/stt/src/session.rs` — same `append/commit/clear/close` + `committed/transcript/error` events. Keep `PASEO_STT_BATCH_COMMIT_EVERY_SECONDS` env behavior as `ASCUTA_STT_COMMIT_SECONDS` |
| `packages/server/src/server/speech/providers/local/sherpa/silero-vad-provider.ts` + `silero-vad-session.ts` + `assets/silero_vad.onnx` | Turn detection / silence trim, no-speech skip | `crates/audio/src/vad.rs` — same bundled model, same thresholds |
| `packages/server/src/server/speech/providers/local/sherpa/sherpa-onnx-node-loader.ts` + `sherpa-runtime-env.ts` | Load native addon, platform pkg resolution, Linux RUNPATH patch | `crates/stt/src/loader.rs` — link `sherpa-onnx-c-api` directly (no Node loader needed), keep platform-matrix error messages |
| `packages/server/src/server/speech/providers/local/worker-process.ts` + `worker-client.ts` | Isolated inference process (`Paseo Voice`), IPC `session.create/append/commit`, crash isolation | `crates/stt/src/worker.rs` — dedicated dictation thread (or child process if native crashes must be isolated). Keep `requiredSampleRate` handshake |
| `packages/server/src/server/agent/stt-manager.ts` | Resample to model rate, 1s append chunks, final-timeout 120s, low-confidence filter (`avgLogprob < -3.0` → `""`) | `crates/stt/src/manager.rs` — same pipeline. Whisper path has no logprobs in sherpa-onnx → use `no-speech-prob` / energy gate instead |
| `packages/server/src/server/agent/stt-debug.ts` | Optional `STT_DEBUG_AUDIO_DIR` raw dump | Keep as `ASCUTA_DEBUG_AUDIO_DIR`, off by default |
| `packages/app/src/hooks/use-dictation-audio-source.web.ts` | `getUserMedia` mono + NS/EC/AGC → `ScriptProcessor(4096)` → RMS volume → resample to 16k PCM16 → ~1s base64 chunks | `crates/audio/src/capture.rs` — CPAL capture, no inference in callback, ring buffer, downmix + resample to 16k mono (same as `audio/resample.rs` in LocalFlow). Volume callback for HUD |
| `packages/app/src/dictation/dictation-stream-sender.ts` + `use-dictation.ts` | Ordered `seq` buffer, `start/send/finish/cancel`, reconnect replay, partials | In-process equivalent: `crates/core` state machine (`idle→recording→decoding→injecting`), no WS needed for v1 |
| `packages/server/src/server/speech/speech-runtime.ts` | Provider reconciliation, readiness (`ready/models_missing/downloading`), background download + monitor tick | `crates/core/src/readiness.rs` — same states, same auto-download UX |

Do not port: OpenAI STT/TTS (`providers/openai/*`), TTS/Kokoro (defer to v2), daemon WS protocol, Expo/Electron UI.

## 3. Stack decision: Rust (recommended)

| Option | Sherpa-onnx fit | Hotkey/audio/inject | Verdict |
|---|---|---|---|
| **Rust + Tauri 2 (hybrid)** | `sherpa-rs` / C-API bind, proven by LocalFlow + OpenWhispr clones | Pure-Rust `global-hotkey`, `cpal`, `enigo`/`arboard`, `muda`/`ksni` tray, `winit` HUD | **Pick this (hybrid).** Single binary, CPU+GPU features. Tauri webview is settings/history window ONLY — tray, HUD, hotkey, injection live in Rust core (Tauri overlay/hotkey/tray are fragile on Wayland: no always-on-top protocol, `global-shortcut` plugin silent on Wayland GNOME, tray missing in dev/.deb) |
| Go (Wails) | `sherpa-onnx-go` exists but thin, CGO pain on mac universal + Win | Hotkey/audio libs weaker, injection hacky | Reject for v1 |
| C++ (Qt/wx) | Native sherpa-onnx C++ API, fastest | Most code by hand, packaging heavy | Only if team is C++-native |

Rule: UI never runs inference. Rust core owns tray, HUD overlay, hotkey, audio/VAD/STT/state/injection; Tauri React window only for settings + history (normal window, no always-on-top / click-through needed).

## 4. Architecture

```text
Rust core (tray daemon via muda/ksni, winit HUD pill, state machine, settings.json, SQLite history)
        ├── hotkey: global-hotkey (X11 grab) / XDG GlobalShortcuts portal wl_register_all (Wayland) / native mac+Win
        ├── audio: cpal capture → ring → 16k resample → Silero VAD trim
        ├── stt worker thread: sherpa-onnx (Parakeet TDT / Whisper ONNX, CPU default)
        ├── post: deterministic tidy (fillers, spoken punctuation, caps) — port of Paseo assemble + LocalFlow rules
        ├── inject: clipboard paste → SendInput/keystroke fallback, restore clipboard
        └── settings UI: Tauri webview window (React+TS: settings, history) — normal window only, IPC events only
```

Inference runs fully on-device (ONNX Runtime CPU default). GPU is opt-in per OS (see §6).

## 5. Models (sherpa-onnx only, v1)

| ID | Type | Size | Langs | Default for |
|---|---|---|---|---|
| `parakeet-tdt-0.6b-v2-int8` | NeMo TDT | ~640MB | EN | English accuracy + punctuation (Paseo default, keep) |
| `parakeet-tdt-0.6b-v3-int8` | NeMo TDT | ~670MB | 25 EU | Multilingual EU |
| `whisper-base-int8` | encoder-decoder | ~150MB | 99+ | Non-EU langs, fallback |
| `whisper-large-v3-turbo-int8` | distilled | ~1GB | 99+ | Accuracy-first (warn on RAM) |
| `whisper-small-int8` | encoder-decoder | ~500MB | 99+ | DEMOTED post-v1: sherpa-onnx CPU RTF 1.369 (>realtime), bench locally before rostering |
| `silero_vad` (bundled) | VAD | ~2MB | — | Always |

Punctuation: Parakeet TDT = built-in. Whisper = post-process tidy (no extra model in v1).
License note: Parakeet `CC-BY-4.0` (attribute), Whisper `MIT`, Moonshine intentionally out of v1 (only if streaming partials demanded later).

## 6. CPU default / GPU optional

* Default everywhere: `provider="cpu"`, STT `numThreads=physical cores (cap 8)` / laptop-battery profile cap 4, VAD `numThreads=1`, `inter_op=1` + `ORT_SEQUENTIAL`, int8 models only. No CUDA/Metal/CoreML code path compiled in by default. (Rationale: 8 threads ~20% faster than 4 on Parakeet; other runtime knobs measured 1–4% each. Target: modern 4–8 core CPUs, `<1s` for 5s burst via Parakeet only.)
* Opt-in (settings toggle, restart worker): Windows/Linux NVIDIA → ORT CUDA execution provider; macOS → CoreML/Metal path via sherpa-onnx build flag; Intel → OpenVINO encoder for Whisper. Ship as separate feature builds (`--features cuda`) so base binary stays lean.
* Guardrails: disable ORT arena in low-RAM containers, 20–30s windowed decode for long files (Parakeet ONNX positional limit ~200s), single window resident at a time.

## 7. Desktop flow (Whisper Flow parity)

1. Idle in tray. User hits hotkey (default: hold `Ctrl+B` Win/Linux, hold `Fn` or `Option+Space` mac; customizable, incl. double-tap hands-free).
2. HUD overlay shows level. Audio buffered with 250ms preroll.
3. Release → VAD trims silence → if no-speech, abort (no paste).
4. STT decode on worker thread (Paseo `STTManager` logic). Target `<1s` for 5s burst on modern laptop CPU.
5. Tidy → dictionary/snippet replace → inject at cursor → save to SQLite history.
6. `Command mode` (v1.1): highlighted text + hotkey + spoken instruction → local tidy only (no LLM in v1).

## 8. UI (minimal, v1 — hybrid: native Rust for OS-integrated parts, Tauri webview for settings only)

* `Settings` (Tauri webview, normal window): model picker (radio per §5 + download state/progress), language (`auto/en/...`), hotkey recorder (hold/toggle/hands-free), mic picker + test meter, `preserve_clipboard` toggle, launch-at-login.
* `History` (same Tauri window, second view): list of past dictations (copy/re-insert/delete), backed by SQLite. Off-by-default audio dump toggle.
* `HUD` (pure Rust `winit` window, NOT a Tauri webview): floating level + `listening/decoding` state, small pill, transparent + click-through (`set_cursor_hittest`; gtk-layer-shell on Wayland; NSPanel-style float on macOS for fullscreen apps). Fixed max size up front (some Wayland compositors ignore resize on mapped layer surfaces); show/hide via unmap→remap so position switches don't drop the surface. Draggable when opted in.
* `Tray` (pure Rust `muda`/`ksni`, NOT Tauri tray): idle/recording/decoding icon states; verified visible on GNOME Wayland + KDE (Tauri tray is missing in dev/.deb on Wayland GNOME).
* `Hotkey` (pure Rust `global-hotkey` + portal binding, NOT `tauri-plugin-global-shortcut`): X11 direct grab; Wayland via single `wl_register_all()` call with stable portal ID + app ID (user confirms once in compositor dialog; rebind handled in system settings, broken on GNOME 48 — document 49+).
* No account, no cloud fields in v1.

## 9. Repo layout (to create)

```text
ascuta-desktop/
├── PLAN.md (this file)
├── src-tauri/ (Rust: core, audio, stt, vad, models, inject, hotkey, store)
│   ├── src/audio/ (device, capture, ring_buffer, wav, resample)
│   ├── src/asr/ (recognizer, session, manager)   # ← Paseo port
│   ├── src/vad/                                   # ← Paseo port
│   ├── src/models/ (catalog, downloader)          # ← Paseo port
│   ├── src/hotkey/ src/overlay.rs src/injection/ src/normalization/
│   └── models/  (gitignored runtime downloads; only silero .onnx bundled)
├── src/ (React settings + history window only — no HUD, no tray, no hotkey code here)
└── scripts/ (model fetch, installer build per OS)
```

## 10. Milestones

* **M0 scaffold (Linux-first gate — must pass before any STT work):** native Rust tray visible on GNOME Wayland + KDE + X11 → native hotkey fires on all three (portal dialog on Wayland) → native `winit` HUD pill shows/hides with level stub → Tauri settings window persists `settings.json` → paste `hello` into focused app. If tray/HUD/hotkey spike fails on Wayland, keep Tauri for settings only and re-implement the failing part natively — never debug webview compositors.
* **M1 audio:** CPAL capture → 16k WAV dump → HUD meter → VAD trim (port `vad/*`).
* **M2 Parakeet:** catalog+downloader port → offline decode `parakeet-v2-int8` → inject (CPU). Parity test vs Paseo `speech:transcribe:local` script on same WAV.
* **M3 Whisper:** add `whisper-base-int8` + `whisper-large-v3-turbo-int8` ONNX via same `OfflineRecognizer` path + tidy post-proc. Language fallback routing (non-EU → Whisper).
* **M4 polish:** history SQLite, snippets/dictionary, hands-free, installers (NSIS/dmg/AppImage), signed builds.
* **M5 (defer):** streaming partials (Moonshine eval), local LLM cleanup (Qwen3-0.6B via llama.cpp), TTS.

## 11. Risks

* Global hotkey + text injection differ per OS (Win `WH_KEYBOARD_LL`/`SendInput`, mac Accessibility+CGEvent, Linux X11 `xdotool` vs Wayland portal limits) — abstract behind `Hotkey`/`Injector` traits with fake impls for CI.
* sherpa-onnx native build matrix (win/mac-arm/mac-x64/linux-x64) — pin version, prebuild, keep `sherpa-runtime-env` equivalent.
* Parakeet RAM (~1.2–1.6GB peak) — windowed decode + int8-only in v1.
* Wayland injection — document X11-first, Wayland via clipboard + manual paste fallback.

## 12. Phone bridge (Android → desktop)

The Ascuta Android app already transcribes on-device; this feature lets it type
the finished transcript on the desktop instead of the phone. The desktop is the
only new listener — no cloud, audio stays on the phone.

* **Daemon:** `src-tauri/src/bridge.rs` runs a `tiny_http` server on its own
  thread (no async runtime in core). Routing/auth is a pure `route()` function
  so `--check`/unit tests need no sockets; requests become `OsEvent::RemoteText`
  and reuse the same injector as local dictation (`inject_remote`).
* **API:** `GET /v1/ping` and `POST /v1/text` (`{"text","enter"}`), both
  requiring `Authorization: Bearer <token>`. Token compare is constant-time and
  an unconfigured (empty) token denies everyone.
* **Pairing:** QR of `ascuta://pair?v=1&host=…&port=…&token=…&name=…`
  (`src-tauri/src/pairing.rs`). 256-bit random token persisted in
  `settings.json` (`bridge_enabled`, `bridge_port=8737`, `bridge_token`).
  `ascuta-desktop --pair` prints the QR to the terminal and serves headless
  until Ctrl+C — the minimum setup, no tray/HUD/webview needed.
* **Android:** `PhoneBridge.kt` parses the same URI, stores the pairing in
  `AppSettings`, and posts each finished transcript from `finishDictation`
  (fire-and-forget on a daemon thread; short timeouts). Pairable from Home →
  "Send to computer" with the ZXing QR scanner.
* **Tests:** Rust unit tests cover routing/auth/pairing plus a loopback
  end-to-end; `--check` covers the pure logic. Android
  `PhoneBridgeTest` covers URI round-trips and the client against MockWebServer.

