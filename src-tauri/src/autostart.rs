//! "Launch at login":
//! * Linux — XDG autostart entry (`~/.config/autostart/ascuta-desktop.desktop`);
//! * macOS — LaunchAgent plist (`~/Library/LaunchAgents/eu.ascuta.desktop.plist`);
//! * Windows — `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use std::path::{Path, PathBuf};

use anyhow::Result;

fn entry_path(config_dir: &Path) -> PathBuf {
    config_dir.join("autostart").join("ascuta-desktop.desktop")
}

fn entry_contents(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Ascuta Desktop\n\
         Comment=Private offline dictation\nExec=\"{}\"\nTerminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exe.display()
    )
}

/// Create or remove the XDG autostart entry under `config_dir`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn apply_in(config_dir: &Path, enabled: bool, exe: &Path) -> Result<()> {
    let path = entry_path(config_dir);
    if enabled {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, entry_contents(exe))?;
    } else if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// macOS LaunchAgent contents (`RunAtLoad`, no keep-alive: quitting from the
/// tray must stay quit).
pub fn launch_agent_plist(exe: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\"><dict>\n\
         <key>Label</key><string>eu.ascuta.desktop</string>\n\
         <key>ProgramArguments</key><array><string>{}</string></array>\n\
         <key>RunAtLoad</key><true/>\n\
         </dict></plist>\n",
        exe.display()
    )
}

pub fn apply(enabled: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    #[cfg(target_os = "linux")]
    {
        let config = dirs::config_dir().ok_or_else(|| anyhow::anyhow!("no config dir"))?;
        apply_in(&config, enabled, &exe)
    }
    #[cfg(target_os = "macos")]
    {
        let dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("no home dir"))?
            .join("Library")
            .join("LaunchAgents");
        let path = dir.join("eu.ascuta.desktop.plist");
        if enabled {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&path, launch_agent_plist(&exe))?;
        } else if path.exists() {
            std::fs::remove_file(&path)?;
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let run = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            winreg::enums::KEY_SET_VALUE,
        )?;
        if enabled {
            run.set_value("Ascuta", &format!("\"{}\"", exe.display()))?;
        } else {
            let _ = run.delete_value("Ascuta"); // absent is fine
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (enabled, exe);
        anyhow::bail!("launch at login is not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_agent_has_label_and_exe() {
        let p = launch_agent_plist(Path::new("/Applications/Ascuta.app/Contents/MacOS/ascuta"));
        assert!(p.contains("<string>eu.ascuta.desktop</string>"));
        assert!(p.contains("/Applications/Ascuta.app/Contents/MacOS/ascuta"));
        assert!(p.contains("<key>RunAtLoad</key><true/>"));
    }

    #[test]
    fn creates_and_removes_entry() {
        let d = tempfile::tempdir().unwrap();
        apply_in(d.path(), true, Path::new("/opt/ascuta/ascuta-desktop")).unwrap();
        let txt = std::fs::read_to_string(entry_path(d.path())).unwrap();
        assert!(txt.contains("Exec=\"/opt/ascuta/ascuta-desktop\""));
        apply_in(d.path(), false, Path::new("/x")).unwrap();
        assert!(!entry_path(d.path()).exists());
        // removing when absent is fine
        apply_in(d.path(), false, Path::new("/x")).unwrap();
    }
}
