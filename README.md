# Ascuta Desktop

Compagno per desktop di **Ascuta** (Android). L'app sul telefono trascrive la
voce **sul dispositivo**; Ascuta Desktop riceve il testo dal telefono e lo
**digita al computer** come se fosse stato scritto con la tastiera.

Trascrizione audio e modelli restano sul telefono: al computer viaggia solo il
testo, sulla rete locale. Niente cloud, niente account.

> **Stato: M0 (scheletro).** La *phone bridge* funziona ed è provabile.
> La trascrizione locale sul desktop (modelli sherpa-onnx, hotkey push-to-talk,
> HUD) è in sviluppo — vedi [`PLAN.md`](PLAN.md).

---

## Cosa fa oggi

- **Phone bridge** — riceve le trascrizioni dall'app Ascuta via Wi-Fi e le
  digita nel campo che ha il fuoco. Abbinamento con **codice QR**.
- **`--pair`** — modalità headless: stampa il QR nel terminale e resta in
  ascolto, senza tray né interfaccia. È il modo più semplice per provare.
- **`--check`** — autotest senza display, audio o rete.
- Tray, hotkey globale (`Ctrl+B`) e HUD: scheletro già presente.

---

## Requisiti (Linux)

- Linux **x86_64**.
- Per **digitare** il testo serve una sessione **X11**. Su Wayland l'iniezione
  da tastiera non è disponibile: il testo resta negli appunti e va incollato a
  mano (per questo la modalità `--pair` resta comunque utile).
- Consigliati per gli appunti: `xclip` (X11) oppure `wl-clipboard` (Wayland).
- Il binario precompilato richiede solo `libc`; X11/Wayland vengono caricati a
  runtime, quindi non serve installare librerie di sviluppo.

---

## 1. Scaricare il progetto

### Opzione A — Release precompilata (consigliata)

1. Apri la pagina delle release:
   **<https://github.com/emavgl/ascuta-desktop/releases>**
2. Scarica `ascuta-desktop-v0.1.0-linux-x86_64.tar.gz`
   (e, se vuoi, il file `.sha256` per verificare l'integrità).

```sh
# verifica l'integrità (facoltativa ma consigliata)
sha256sum -c ascuta-desktop-v0.1.0-linux-x86_64.tar.gz.sha256

# estrai
tar -xzf ascuta-desktop-v0.1.0-linux-x86_64.tar.gz
```

### Opzione B — Dal sorgente

```sh
# strumenti di base (linker C + git)
sudo apt update && sudo apt install -y build-essential git

# Rust (se non è già installato)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# codice
git clone https://github.com/emavgl/ascuta-desktop.git
cd ascuta-desktop
cargo build --release
```

Il binario è in `target/release/ascuta-desktop`.

---

## 2. Installare su Linux

Con il pacchetto precompilato:

```sh
mkdir -p ~/.local/bin
install -m755 \
  ascuta-desktop-v0.1.0-linux-x86_64/ascuta-desktop \
  ~/.local/bin/ascuta-desktop
```

Assicurati che `~/.local/bin` sia nel `PATH` (di solito lo è già):

```sh
echo "$PATH" | tr ':' '\n' | grep -q "$HOME/.local/bin" \
  || echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
```

Dal sorgente, invece:

```sh
install -m755 target/release/ascuta-desktop ~/.local/bin/ascuta-desktop
```

- **Avvio normale:** `ascuta-desktop`
- **File impostazioni:** `~/.config/ascuta/settings.json`
  (sovrascrivibile con la variabile `ASCUTA_CONFIG_FILE`).

---

## 3. Come provarlo

### 3.1 Autotest (nessun display richiesto)

```sh
ascuta-desktop --check
```

Deve terminare con:

```
ascuta --check: OK (press/release/paste-hello/phone-bridge/settings-roundtrip)
```

### 3.2 Provare la phone bridge (il caso d'uso principale)

1. **Sul computer** avvia la modalità abbinamento:

   ```sh
   ascuta-desktop --pair
   ```

   Stampa un **codice QR** nel terminale e resta in ascolto sulla porta `8737`.
   Lascia il terminale aperto; `Ctrl+C` per fermarlo.

2. **Sul telefono** apri Ascuta e vai in
   **Home → Invia al computer** (attiva l'interruttore) → **Abbina un computer**,
   poi **scansiona il QR** mostrato dal computer.

3. **Dettare** con Ascuta (bolla, tastiera o popup). Il testo finale viene
   digitato sul computer nella finestra che ha il fuoco.

4. **Test connessione** (Home → Invia al computer) verifica che il computer sia
   raggiungibile.

Note:

- Telefono e computer devono essere sulla **stessa rete Wi-Fi**; il firewall
  deve permettere la porta `8737`.
- La sicurezza è locale: bind sulla LAN e token generato nel QR. Nessun dato
  esce dalla rete locale.
- Serve una versione recente dell'app Ascuta che includa la bridge (repo
  `ascuta`).

### 3.3 Modalità normale (desktop con display)

```sh
ascuta-desktop
```

Avvia tray, hotkey globale (`Ctrl+B`, premere e tenere premuto) e HUD; la bridge
parte solo se `bridge_enabled` è `true` in `settings.json`. Se tray o hotkey non
sono disponibili (es. Wayland, GNOME senza estensione), il programma avvisa e
continua.

---

## 4. Problemi comuni

| Sintomo | Causa / Soluzione |
|---|---|
| Il testo non viene digitato, compare `no key backend` | Sessione Wayland o senza X11: il testo resta negli appunti, incollalo manualmente. Per la digitazione usa X11. |
| `address already in use` all'avvio | Porta `8737` occupata: cambia `bridge_port` in `~/.config/ascuta/settings.json` e riavvia (anche il QR cambierà porta). |
| Nessuna icona nel tray su GNOME | Installa l'estensione *AppIndicator* oppure usa `--pair`. |
| Il QR non si vede bene | Il QR usa caratteri Unicode a blocchi: serve un terminale con font monospace. |
| Verificare il download | `sha256sum -c ascuta-desktop-*.tar.gz.sha256` |

---

## 5. Sviluppo

```sh
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --check   # gate headless
```

Architettura, modelli e roadmap: [`PLAN.md`](PLAN.md).

Le release sono prodotte automaticamente dalla GitHub Action
[`.github/workflows/release.yml`](.github/workflows/release.yml): pubblicando un
tag (`git tag v0.1.0 && git push origin v0.1.0`) viene creata una release con il
binario Linux precompilato e il checksum tra gli asset.

---

## Licenza

MIT.
