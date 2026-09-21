//! App settings: load/save JSON at the per-OS config dir.
//!
//! Linux:   ~/.config/ascuta/settings.json
//! macOS:   ~/Library/Application Support/ascuta/settings.json
//! Windows: %APPDATA%/ascuta/settings.json
//! Override for tests/CI: `ASCUTA_CONFIG_FILE`.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// STT model id, see PLAN §5 (e.g. `parakeet-tdt-0.6b-v2-int8`).
    pub model: String,
    /// BCP-47 or `auto`.
    pub language: String,
    /// Human-readable hotkey, e.g. `Ctrl+B` (hold-to-talk).
    pub hotkey: String,
    pub preserve_clipboard: bool,
    pub launch_at_login: bool,
    /// Phone bridge (receive transcripts over the LAN). Opt-in.
    pub bridge_enabled: bool,
    pub bridge_port: u16,
    /// 256-bit hex token from the QR pairing payload. Generated on first use.
    pub bridge_token: String,
}

impl Settings {
    /// Generate a pairing token if there is none. Returns `true` when the
    /// settings changed (caller should persist them).
    pub fn ensure_bridge_token(&mut self) -> bool {
        if self.bridge_token.is_empty() {
            self.bridge_token = crate::pairing::generate_token().unwrap_or_default();
            true
        } else {
            false
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: "parakeet-tdt-0.6b-v2-int8".to_string(),
            language: "auto".to_string(),
            hotkey: "Ctrl+B".to_string(),
            preserve_clipboard: true,
            launch_at_login: false,
            bridge_enabled: false,
            bridge_port: DEFAULT_BRIDGE_PORT,
            bridge_token: String::new(),
        }
    }
}

/// Default phone-bridge port (see PLAN §12).
pub const DEFAULT_BRIDGE_PORT: u16 = 8737;

pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn load() -> Result<Settings> {
        let s = Self::with_path(Self::path()).read()?;
        Ok(normalize(s))
    }

    pub fn read(&self) -> Result<Settings> {
        if !self.path.exists() {
            return Ok(Settings::default());
        }
        let json = std::fs::read_to_string(&self.path)?;
        Ok(normalize(serde_json::from_str(&json)?))
    }

    pub fn save(&self, settings: &Settings) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(settings)?;
        std::fs::write(&self.path, json)?;
        Ok(())
    }

    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("ASCUTA_CONFIG_FILE") {
            return PathBuf::from(p);
        }
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("ascuta").join("settings.json")
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }
}

/// Unknown model ids fall back to the Paseo default instead of bricking
/// the daemon after a catalog rename.
fn normalize(mut s: Settings) -> Settings {
    if !crate::models::is_known_model(&s.model) {
        s.model = crate::models::DEFAULT_STT_MODEL.to_string();
    }
    if s.language.trim().is_empty() {
        s.language = "auto".to_string();
    }
    if s.hotkey.trim().is_empty() {
        s.hotkey = "Ctrl+B".to_string();
    }
    if s.bridge_port == 0 {
        s.bridge_port = DEFAULT_BRIDGE_PORT;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bridge_is_off() {
        let s = Settings::default();
        assert!(!s.bridge_enabled);
        assert_eq!(s.bridge_port, DEFAULT_BRIDGE_PORT);
        assert!(s.bridge_token.is_empty());
    }

    #[test]
    fn ensure_token_generates_once_and_is_stable() {
        let mut s = Settings::default();
        assert!(s.ensure_bridge_token());
        let first = s.bridge_token.clone();
        assert_eq!(first.len(), 64);
        assert!(!s.ensure_bridge_token());
        assert_eq!(s.bridge_token, first);
    }

    #[test]
    fn round_trips_bridge_fields() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::with_path(dir.path().join("settings.json"));
        let mut s = Settings {
            bridge_enabled: true,
            bridge_port: 9001,
            ..Settings::default()
        };
        s.ensure_bridge_token();
        store.save(&s).unwrap();
        assert_eq!(store.read().unwrap(), s);
    }

    #[test]
    fn zero_port_normalized() {
        let s = normalize(Settings {
            bridge_port: 0,
            ..Settings::default()
        });
        assert_eq!(s.bridge_port, DEFAULT_BRIDGE_PORT);
    }
}
