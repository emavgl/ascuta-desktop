# Tauri settings window — desktop wiring (not done in headless CI)

The Rust core (`src-tauri/src/`) builds anywhere. The Tauri webview needs
system webview libs, which headless CI boxes (and this dev container) lack.
Wire it on a Linux desktop:

```sh
# system deps (Debian/Ubuntu/Mint)
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf xdg-utils wl-clipboard

# tauri crates (from src-tauri/)
# NOTE: no tray-icon / global-shortcut features — tray, HUD, hotkey stay
# in Rust core per PLAN §8. Tauri is the settings/history window only.
cargo add tauri
cargo add tauri-build --build
cargo add tauri-plugin-opener
```

Then in `src-tauri/src/main.rs`:

1. `tauri::Builder::default()` hosting the existing winit loop integration —
   keep tray/HUD/hotkey in Rust core (PLAN §8); Tauri owns the `settings`
   window only.
2. Commands bridging `settings::Store`:
   - `get_settings() -> Settings`
   - `save_settings(settings: Settings)`
3. `npm i -D @tauri-apps/cli @tauri-apps/api && npx tauri dev`.

M0 gate (PLAN §10, Linux-first): tray visible on GNOME Wayland + KDE + X11
→ hotkey fires → HUD pill shows → settings persists → paste `hello`.
