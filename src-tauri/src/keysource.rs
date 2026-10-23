//! Raw global key events (down/up) feeding the chord tracker.
//!
//! * Linux: `evdev` — reads `/dev/input/event*`, works on X11 *and* Wayland,
//!   needs the user in the `input` group.
//! * macOS/Windows: `rdev` global listener (Accessibility permission on mac).
//!
//! Events are observed, never swallowed, so a chord like `Ctrl+B` also
//! reaches the focused app; modifier-only chords are the recommended kind.

use std::sync::mpsc::Sender;

use anyhow::Result;

use crate::chord::KeyName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawKey {
    pub key: KeyName,
    pub down: bool,
}

/// Start listening on background threads. Returns how many keyboards are
/// being watched; errors when none can be opened.
#[cfg(target_os = "linux")]
pub fn start(tx: Sender<RawKey>) -> Result<usize> {
    match start_direct(tx.clone()) {
        Ok(n) => Ok(n),
        Err(e) => {
            // Added to `input` but this session predates it: read the
            // keyboards from a helper started via `sg input`. The main
            // process keeps its normal groups (the desktop portals reject
            // processes whose primary group was switched).
            if in_input_group_db() {
                if let Ok(n) = start_via_helper(tx) {
                    return Ok(n);
                }
            }
            Err(e)
        }
    }
}

#[cfg(target_os = "linux")]
fn in_input_group_db() -> bool {
    let user = std::env::var("USER").unwrap_or_default();
    std::fs::read_to_string("/etc/group")
        .unwrap_or_default()
        .lines()
        .any(|l| {
            let mut f = l.split(':');
            f.next() == Some("input") && f.nth(2).is_some_and(|m| m.split(',').any(|u| u == user))
        })
}

/// `--key-helper` entry point: print `ready N`, then `<Key> <1|0>` lines.
#[cfg(target_os = "linux")]
pub fn run_helper() -> Result<()> {
    use std::io::Write;
    let (tx, rx) = std::sync::mpsc::channel();
    let n = start_direct(tx)?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "ready {n}")?;
    out.flush()?;
    // SAFETY: getppid has no preconditions.
    let parent = unsafe { libc::getppid() };
    loop {
        match rx.recv_timeout(std::time::Duration::from_secs(1)) {
            Ok(k) => {
                if writeln!(out, "{} {}", k.key, u8::from(k.down)).is_err() || out.flush().is_err() {
                    return Ok(());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // SAFETY: as above.
                if unsafe { libc::getppid() } != parent {
                    return Ok(());
                }
            }
            Err(_) => return Ok(()),
        }
    }
}

#[cfg(target_os = "linux")]
fn start_via_helper(tx: Sender<RawKey>) -> Result<usize> {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let exe = std::env::current_exe()?;
    let quoted = exe.to_string_lossy().replace('\'', "'\\''");
    let mut child = Command::new("sg")
        .args(["input", "-c", &format!("'{quoted}' --key-helper")])
        .stdout(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no helper stdout"))?;
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<usize>();
    std::thread::Builder::new()
        .name("ascuta-keys-helper".into())
        .spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(n) = line.strip_prefix("ready ") {
                    let _ = ready_tx.send(n.parse().unwrap_or(1));
                    continue;
                }
                let mut it = line.split(' ');
                let (Some(name), Some(state)) = (it.next(), it.next()) else {
                    continue;
                };
                if let Some((key, _)) = crate::chord::KEYS.iter().find(|(k, _)| *k == name) {
                    if tx.send(RawKey { key, down: state == "1" }).is_err() {
                        break;
                    }
                }
            }
            let _ = child.kill();
        })?;
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .map_err(|_| anyhow::anyhow!("key helper did not start"))
}

#[cfg(target_os = "linux")]
fn start_direct(tx: Sender<RawKey>) -> Result<usize> {
    use evdev::{EventType, KeyCode};

    let mut watched = 0usize;
    let mut denied = false;
    for entry in std::fs::read_dir("/dev/input")? {
        let path = entry?.path();
        let is_event = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("event"));
        if !is_event {
            continue;
        }
        let dev = match evdev::Device::open(&path) {
            Ok(d) => d,
            Err(e) => {
                denied |= e.kind() == std::io::ErrorKind::PermissionDenied;
                continue;
            }
        };
        // A keyboard has letter keys; skips mice, power buttons, etc.
        let is_keyboard = dev
            .supported_keys()
            .is_some_and(|k| k.contains(KeyCode::KEY_A) && k.contains(KeyCode::KEY_LEFTCTRL));
        if !is_keyboard {
            continue;
        }
        watched += 1;
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("ascuta-keys".into())
            .spawn(move || {
                let mut dev = dev;
                loop {
                    let events = match dev.fetch_events() {
                        Ok(e) => e,
                        Err(_) => return, // unplugged
                    };
                    for ev in events {
                        if ev.event_type() != EventType::KEY || ev.value() == 2 {
                            continue; // not a key, or auto-repeat
                        }
                        if let Some(key) = crate::chord::key_from_evdev(ev.code()) {
                            let down = ev.value() == 1;
                            if tx.send(RawKey { key, down }).is_err() {
                                return;
                            }
                        }
                    }
                }
            })?;
    }
    if watched == 0 {
        if denied {
            anyhow::bail!(
                "no permission to read keyboards (/dev/input). Run \
                 `sudo usermod -aG input $USER`, then log out and back in"
            );
        }
        anyhow::bail!("no keyboard found in /dev/input");
    }
    Ok(watched)
}

#[cfg(not(target_os = "linux"))]
pub fn start(tx: Sender<RawKey>) -> Result<usize> {
    std::thread::Builder::new()
        .name("ascuta-keys".into())
        .spawn(move || {
            let res = rdev::listen(move |ev| {
                let (key, down) = match ev.event_type {
                    rdev::EventType::KeyPress(k) => (k, true),
                    rdev::EventType::KeyRelease(k) => (k, false),
                    _ => return,
                };
                if let Some(name) = name_from_rdev(&format!("{key:?}")) {
                    let _ = tx.send(RawKey { key: name, down });
                }
            });
            if let Err(e) = res {
                eprintln!("ascuta: global key listener stopped: {e:?}");
            }
        })?;
    Ok(1)
}

/// Map rdev's `Debug` key names (e.g. `KeyA`, `Num1`, `ControlLeft`).
#[cfg(not(target_os = "linux"))]
fn name_from_rdev(dbg: &str) -> Option<KeyName> {
    let canonical: &str = match dbg {
        "ControlLeft" => "LeftCtrl",
        "ControlRight" => "RightCtrl",
        "ShiftLeft" => "LeftShift",
        "ShiftRight" => "RightShift",
        "Alt" => "LeftAlt",
        "AltGr" => "RightAlt",
        "MetaLeft" => "LeftWin",
        "MetaRight" => "RightWin",
        "Return" => "Enter",
        "UpArrow" => "Up",
        "DownArrow" => "Down",
        "LeftArrow" => "Left",
        "RightArrow" => "Right",
        "Function" => "Fn",
        "BackQuote" => "Grave",
        "LeftBracket" => "LeftBracket",
        "RightBracket" => "RightBracket",
        "SemiColon" => "Semicolon",
        "Quote" => "Apostrophe",
        "BackSlash" => "Backslash",
        "Dot" => "Period",
        s if s.len() == 4 && s.starts_with("Key") => &s[3..],
        s if s.len() == 4 && s.starts_with("Num") => &s[3..],
        s => s,
    };
    crate::chord::KEYS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(canonical))
        .map(|(n, _)| *n)
}
