// M0 stub: talks to the Tauri command bridge once the `tauri` crate +
// JS API are wired on a desktop with webview libs (see TAURI_SETUP.md).
// Until then, edit settings.json directly:
//   Linux: ~/.config/ascuta/settings.json
async function load() {
  if (!window.__TAURI__) return;
  const { invoke } = window.__TAURI__.core;
  const s = await invoke("get_settings");
  document.querySelector(`input[name="model"][value="${s.model}"]`)?.click();
  document.getElementById("language").value = s.language;
  document.getElementById("hotkey").textContent = s.hotkey;
  document.getElementById("preserve_clipboard").checked = s.preserve_clipboard;
  document.getElementById("launch_at_login").checked = s.launch_at_login;
}

async function save() {
  const model = document.querySelector('input[name="model"]:checked')?.value;
  const settings = {
    model,
    language: document.getElementById("language").value,
    hotkey: document.getElementById("hotkey").textContent,
    preserve_clipboard: document.getElementById("preserve_clipboard").checked,
    launch_at_login: document.getElementById("launch_at_login").checked,
  };
  if (!window.__TAURI__) {
    document.getElementById("saved").textContent =
      "Tauri bridge not wired yet — edit settings.json directly.";
    return;
  }
  const { invoke } = window.__TAURI__.core;
  await invoke("save_settings", { settings });
  document.getElementById("saved").textContent = "Saved.";
}

document.getElementById("save").addEventListener("click", save);
load();
