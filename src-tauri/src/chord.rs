//! Hotkey chords, Whisper-Flow style: any combination of keys, including
//! **modifier-only** chords (`Ctrl+Win`, `Right Alt`, `Fn`), held to talk.
//!
//! Pure logic (no OS calls): parsing/formatting of chord strings and a
//! tracker that turns raw key up/down events into press/release
//! transitions. Event sources live in `keysource`.
//!
//! Names are case-insensitive and ignore spaces/`-`/`_`: `ctrl+win`,
//! `Right Alt`, `RightCtrl+Space`, `Ctrl+Shift+B`, `F8`, `CapsLock`.
//! `Ctrl`/`Shift`/`Alt`/`Win` match either side; `LeftCtrl`/`RightAlt`/…
//! match one side only.

use std::collections::HashSet;
use std::fmt;

/// Canonical key names with their Linux evdev key codes.
pub const KEYS: &[(&str, u16)] = &[
    ("LeftCtrl", 29), ("RightCtrl", 97), ("LeftShift", 42), ("RightShift", 54),
    ("LeftAlt", 56), ("RightAlt", 100), ("LeftWin", 125), ("RightWin", 126),
    ("A", 30), ("B", 48), ("C", 46), ("D", 32), ("E", 18), ("F", 33), ("G", 34),
    ("H", 35), ("I", 23), ("J", 36), ("K", 37), ("L", 38), ("M", 50), ("N", 49),
    ("O", 24), ("P", 25), ("Q", 16), ("R", 19), ("S", 31), ("T", 20), ("U", 22),
    ("V", 47), ("W", 17), ("X", 45), ("Y", 21), ("Z", 44),
    ("1", 2), ("2", 3), ("3", 4), ("4", 5), ("5", 6), ("6", 7), ("7", 8),
    ("8", 9), ("9", 10), ("0", 11),
    ("F1", 59), ("F2", 60), ("F3", 61), ("F4", 62), ("F5", 63), ("F6", 64),
    ("F7", 65), ("F8", 66), ("F9", 67), ("F10", 68), ("F11", 87), ("F12", 88),
    ("F13", 183), ("F14", 184), ("F15", 185), ("F16", 186), ("F17", 187),
    ("F18", 188), ("F19", 189), ("F20", 190), ("F21", 191), ("F22", 192),
    ("F23", 193), ("F24", 194),
    ("Space", 57), ("Enter", 28), ("Tab", 15), ("Escape", 1), ("Backspace", 14),
    ("CapsLock", 58), ("Insert", 110), ("Delete", 111), ("Home", 102), ("End", 107),
    ("PageUp", 104), ("PageDown", 109), ("Up", 103), ("Down", 108), ("Left", 105),
    ("Right", 106), ("Minus", 12), ("Equal", 13), ("Grave", 41), ("Semicolon", 39),
    ("Comma", 51), ("Period", 52), ("Slash", 53), ("Backslash", 43),
    ("Apostrophe", 40), ("LeftBracket", 26), ("RightBracket", 27),
    ("ScrollLock", 70), ("Pause", 119), ("PrintScreen", 99), ("Menu", 127),
    ("NumLock", 69), ("Fn", 464),
];

pub type KeyName = &'static str;

pub fn key_from_evdev(code: u16) -> Option<KeyName> {
    KEYS.iter().find(|(_, c)| *c == code).map(|(n, _)| *n)
}

pub fn is_modifier(key: &str) -> bool {
    matches!(
        key,
        "LeftCtrl" | "RightCtrl" | "LeftShift" | "RightShift" | "LeftAlt" | "RightAlt"
            | "LeftWin" | "RightWin"
    )
}

fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '_')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn lookup(name: &str) -> Option<KeyName> {
    let q = squash(name);
    KEYS.iter()
        .find(|(n, _)| squash(n) == q)
        .map(|(n, _)| *n)
}

/// One required position of the chord: any of `keys` satisfies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub label: &'static str,
    pub keys: &'static [KeyName],
}

const CTRL: Slot = Slot { label: "Ctrl", keys: &["LeftCtrl", "RightCtrl"] };
const SHIFT: Slot = Slot { label: "Shift", keys: &["LeftShift", "RightShift"] };
const ALT: Slot = Slot { label: "Alt", keys: &["LeftAlt", "RightAlt"] };
const WIN: Slot = Slot { label: "Win", keys: &["LeftWin", "RightWin"] };

fn slot_for(token: &str) -> Option<Slot> {
    let q = squash(token);
    let generic = match q.as_str() {
        "ctrl" | "control" => Some(CTRL),
        "shift" => Some(SHIFT),
        "alt" | "option" | "opt" => Some(ALT),
        "win" | "windows" | "super" | "meta" | "cmd" | "command" => Some(WIN),
        _ => None,
    };
    if generic.is_some() {
        return generic;
    }
    let aliased = match q.as_str() {
        "lctrl" | "leftcontrol" | "ctrlleft" => "LeftCtrl",
        "rctrl" | "rightcontrol" | "ctrlright" => "RightCtrl",
        "lshift" | "shiftleft" => "LeftShift",
        "rshift" | "shiftright" => "RightShift",
        "lalt" | "leftoption" | "altleft" => "LeftAlt",
        "ralt" | "altgr" | "rightoption" | "altright" => "RightAlt",
        "lwin" | "leftmeta" | "leftsuper" | "leftcmd" | "leftcommand" | "metaleft" => "LeftWin",
        "rwin" | "rightmeta" | "rightsuper" | "rightcmd" | "rightcommand" | "metaright" => {
            "RightWin"
        }
        "esc" => "Escape",
        "return" => "Enter",
        "del" => "Delete",
        "pgup" => "PageUp",
        "pgdn" | "pgdown" => "PageDown",
        "caps" => "CapsLock",
        "print" => "PrintScreen",
        "function" => "Fn",
        _ => "",
    };
    let name = if aliased.is_empty() { lookup(token)? } else { lookup(aliased)? };
    Some(Slot {
        label: name,
        keys: KEYS.iter().find(|(n, _)| *n == name).map(|(n, _)| std::slice::from_ref(n))?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    slots: Vec<Slot>,
}

impl Chord {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut slots: Vec<Slot> = Vec::new();
        for token in spec.split('+') {
            if token.trim().is_empty() {
                return Err(format!("empty key in {spec:?}"));
            }
            let slot = slot_for(token).ok_or_else(|| format!("unknown key: {}", token.trim()))?;
            if slots.contains(&slot) {
                return Err(format!("duplicate key: {}", slot.label));
            }
            slots.push(slot);
        }
        if slots.is_empty() {
            return Err("empty hotkey".into());
        }
        if slots.len() > 4 {
            return Err("at most 4 keys".into());
        }
        // Stable, readable order: Ctrl, Shift, Alt, Win, then the rest.
        slots.sort_by_key(|s| {
            let l = s.label;
            if l.ends_with("Ctrl") {
                0
            } else if l.ends_with("Shift") {
                1
            } else if l.ends_with("Alt") {
                2
            } else if l.ends_with("Win") {
                3
            } else {
                4
            }
        });
        Ok(Self { slots })
    }

    /// A lone key you type with (letters, digits, Space, …) would hijack
    /// normal typing; only function-style keys may be used on their own.
    pub fn is_safe(&self) -> bool {
        if self.slots.len() != 1 {
            return true;
        }
        let label = self.slots[0].label;
        if is_modifier(label) || self.slots[0].keys.iter().all(|k| is_modifier(k)) {
            return true;
        }
        let is_fkey = label
            .strip_prefix('F')
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        is_fkey
            || matches!(
                label,
                "Fn" | "CapsLock" | "ScrollLock" | "Pause" | "PrintScreen" | "Insert"
                    | "Menu" | "NumLock"
            )
    }

    /// `ctrl+shift+KeyB`-style spec for the X11 grab fallback, only when the
    /// chord is "modifiers + one regular key" (grabs cannot do modifier-only).
    pub fn legacy_spec(&self) -> Option<String> {
        let (mut mods, mut key) = (Vec::new(), None);
        for s in &self.slots {
            match s.label {
                "Ctrl" | "LeftCtrl" | "RightCtrl" => mods.push("ctrl".to_string()),
                "Shift" | "LeftShift" | "RightShift" => mods.push("shift".to_string()),
                "Alt" | "LeftAlt" | "RightAlt" => mods.push("alt".to_string()),
                "Win" | "LeftWin" | "RightWin" => mods.push("super".to_string()),
                other if key.is_none() => key = Some(other.to_string()),
                _ => return None,
            }
        }
        key.map(|k| {
            mods.push(k);
            mods.join("+")
        })
    }

    pub fn is_satisfied(&self, pressed: &HashSet<KeyName>) -> bool {
        self.slots
            .iter()
            .all(|s| s.keys.iter().any(|k| pressed.contains(k)))
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<&str> = self.slots.iter().map(|s| s.label).collect();
        write!(f, "{}", parts.join("+"))
    }
}

/// Turns raw key events into hold-to-talk press/release transitions.
pub struct Tracker {
    chord: Chord,
    pressed: HashSet<KeyName>,
    active: bool,
}

impl Tracker {
    pub fn new(chord: Chord) -> Self {
        Self {
            chord,
            pressed: HashSet::new(),
            active: false,
        }
    }

    /// `Some(true)` when the chord becomes held, `Some(false)` when released.
    pub fn on_key(&mut self, key: KeyName, down: bool) -> Option<bool> {
        if down {
            self.pressed.insert(key);
        } else {
            self.pressed.remove(key);
        }
        let now = self.chord.is_satisfied(&self.pressed);
        if now != self.active {
            self.active = now;
            Some(now)
        } else {
            None
        }
    }
}

/// Canonical chord string for the keys seen in one recording (used by the
/// settings recorder): left modifiers become generic (`Ctrl`), right ones
/// stay specific (`RightCtrl`), modifiers come first.
pub fn chord_from_keys(keys: &[KeyName]) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for k in keys {
        let label = match *k {
            "LeftCtrl" => "Ctrl",
            "LeftShift" => "Shift",
            "LeftAlt" => "Alt",
            "LeftWin" => "Win",
            other => other,
        };
        if !parts.iter().any(|p| p == label) {
            parts.push(label.to_string());
        }
    }
    let spec = parts.join("+");
    Chord::parse(&spec).ok().map(|c| c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(keys: &[KeyName]) -> HashSet<KeyName> {
        keys.iter().copied().collect()
    }

    #[test]
    fn parses_whisper_flow_style_chords() {
        for (spec, canon) in [
            ("ctrl+win", "Ctrl+Win"),
            ("Ctrl + Super", "Ctrl+Win"),
            ("Right Alt", "RightAlt"),
            ("AltGr", "RightAlt"),
            ("Fn", "Fn"),
            ("F8", "F8"),
            ("Ctrl+Shift+B", "Ctrl+Shift+B"),
            ("B+Ctrl", "Ctrl+B"),
            ("RightCtrl+Space", "RightCtrl+Space"),
            ("Option+Space", "Alt+Space"),
            ("CapsLock", "CapsLock"),
        ] {
            assert_eq!(Chord::parse(spec).unwrap().to_string(), canon, "{spec}");
        }
    }

    #[test]
    fn rejects_bad_chords() {
        for bad in ["", "+", "Ctrl+", "Nope", "Ctrl+Ctrl", "A+B+C+D+E"] {
            assert!(Chord::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn modifier_only_chord_press_release() {
        let mut t = Tracker::new(Chord::parse("Ctrl+Win").unwrap());
        assert_eq!(t.on_key("LeftCtrl", true), None);
        assert_eq!(t.on_key("RightWin", true), Some(true));
        assert_eq!(t.on_key("A", true), None); // extra keys do not matter
        assert_eq!(t.on_key("LeftCtrl", false), Some(false));
        assert_eq!(t.on_key("LeftCtrl", true), Some(true));
    }

    #[test]
    fn legacy_spec_only_with_a_regular_key() {
        let c = |s: &str| Chord::parse(s).unwrap().legacy_spec();
        assert_eq!(c("Ctrl+B").as_deref(), Some("ctrl+B"));
        assert_eq!(c("Ctrl+Shift+Space").as_deref(), Some("ctrl+shift+Space"));
        assert_eq!(c("Ctrl+Win"), None);
        assert_eq!(c("RightAlt"), None);
    }

    #[test]
    fn lone_typing_keys_are_unsafe() {
        for ok in ["F8", "Fn", "CapsLock", "Ctrl+B", "Ctrl+Win", "RightAlt", "Pause"] {
            assert!(Chord::parse(ok).unwrap().is_safe(), "{ok}");
        }
        for bad in ["W", "Space", "5", "Enter", "Tab"] {
            assert!(!Chord::parse(bad).unwrap().is_safe(), "{bad}");
        }
    }

    #[test]
    fn specific_side_only() {
        let c = Chord::parse("Right Alt").unwrap();
        assert!(!c.is_satisfied(&set(&["LeftAlt"])));
        assert!(c.is_satisfied(&set(&["RightAlt"])));
    }

    #[test]
    fn single_key_chord() {
        let mut t = Tracker::new(Chord::parse("F8").unwrap());
        assert_eq!(t.on_key("F8", true), Some(true));
        assert_eq!(t.on_key("F8", true), None); // auto-repeat
        assert_eq!(t.on_key("F8", false), Some(false));
    }

    #[test]
    fn recorder_output_round_trips() {
        assert_eq!(
            chord_from_keys(&["LeftCtrl", "LeftWin"]).as_deref(),
            Some("Ctrl+Win")
        );
        assert_eq!(chord_from_keys(&["RightAlt"]).as_deref(), Some("RightAlt"));
        assert_eq!(
            chord_from_keys(&["LeftShift", "B", "LeftCtrl"]).as_deref(),
            Some("Ctrl+Shift+B")
        );
        assert_eq!(chord_from_keys(&[]), None);
    }

    #[test]
    fn every_table_key_parses() {
        for (name, _) in KEYS {
            assert!(Chord::parse(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn evdev_codes_unique() {
        let mut seen = HashSet::new();
        for (n, c) in KEYS {
            assert!(seen.insert(*c), "duplicate code for {n}");
        }
        assert_eq!(key_from_evdev(29), Some("LeftCtrl"));
    }
}
