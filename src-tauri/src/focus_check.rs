//! "Is there a text cursor to type into?" via AT-SPI (the desktop
//! accessibility bus). Used after a transcription to choose between pasting
//! at the cursor and just copying to the clipboard with a notice.
//!
//! The answer is deliberately three-valued: apps that do not expose
//! accessibility data (some Electron/Chrome builds, games) yield `Unknown`,
//! and the caller then tries to paste anyway instead of dropping the text.

#[cfg(target_os = "linux")]
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// An editable text widget (or terminal) has keyboard focus.
    TextField,
    /// The active window exposes accessibility and nothing editable is focused.
    NoTextField,
    /// Cannot tell (no a11y bus, app does not expose its widgets, timeout).
    Unknown,
}

fn a11y_flag(set: Option<bool>) -> Option<bool> {
    use std::process::Command;
    let base = [
        "--user",
        "org.a11y.Bus",
        "/org/a11y/bus",
        "org.a11y.Status",
        "IsEnabled",
    ];
    match set {
        None => {
            let out = Command::new("busctl")
                .arg("get-property")
                .args(["org.a11y.Bus", "/org/a11y/bus", "org.a11y.Status", "IsEnabled"])
                .arg("--user")
                .output()
                .ok()?;
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .strip_prefix("b ")
                .map(|v| v == "true")
        }
        Some(v) => {
            let ok = Command::new("busctl")
                .arg("set-property")
                .args(["--user", base[1], base[2], base[3], base[4], "b"])
                .arg(if v { "true" } else { "false" })
                .status()
                .is_ok_and(|s| s.success());
            ok.then_some(v)
        }
    }
}

/// Toolkits only expose their widgets once the session's accessibility flag
/// is on (what a screen reader does). Turn it on while Ascuta runs; returns
/// `true` when we changed it, so it can be restored on exit.
pub fn enable_accessibility() -> bool {
    if !cfg!(target_os = "linux") || a11y_flag(None) != Some(false) {
        return false;
    }
    a11y_flag(Some(true)) == Some(true)
}

pub fn restore_accessibility() {
    let _ = a11y_flag(Some(false));
}

/// Blocking probe with a short time budget.
#[cfg(target_os = "linux")]
pub fn focused_text_field() -> Focus {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return Focus::Unknown,
    };
    rt.block_on(async {
        match tokio::time::timeout(Duration::from_millis(900), probe()).await {
            Ok(Ok(f)) => f,
            _ => Focus::Unknown,
        }
    })
}

#[cfg(not(target_os = "linux"))]
pub fn focused_text_field() -> Focus {
    Focus::Unknown
}

#[cfg(target_os = "linux")]
async fn probe() -> anyhow::Result<Focus> {
    use atspi::connection::AccessibilityConnection;
    use atspi::proxy::accessible::ObjectRefExt as _;
    use atspi::{Interface, Role, State};

    let conn = AccessibilityConnection::new().await?;
    let z = conn.connection();
    let root = conn.root_accessible_on_registry().await?;

    let mut saw_active_window = false;
    let mut saw_focused_non_text = false;
    let mut budget = 700usize;

    for app in root.get_children().await? {
        let Ok(app_proxy) = app.as_accessible_proxy(z).await else {
            continue;
        };
        let Ok(windows) = app_proxy.get_children().await else {
            continue;
        };
        for win in windows {
            let Ok(wp) = win.as_accessible_proxy(z).await else {
                continue;
            };
            let Ok(ws) = wp.get_state().await else {
                continue;
            };
            if !ws.contains(State::Active) {
                continue; // background window
            }
            saw_active_window = true;
            let mut stack = vec![win];
            while let Some(node) = stack.pop() {
                if budget == 0 {
                    break;
                }
                budget -= 1;
                let Ok(p) = node.as_accessible_proxy(z).await else {
                    continue;
                };
                let Ok(state) = p.get_state().await else {
                    continue;
                };
                if state.contains(State::Focused) {
                    let ifaces = p.get_interfaces().await.ok();
                    let role = p.get_role().await.ok();
                    let editable = ifaces
                        .as_ref()
                        .is_some_and(|i| i.contains(Interface::EditableText))
                        || matches!(
                            role,
                            Some(
                                Role::Entry
                                    | Role::Text
                                    | Role::PasswordText
                                    | Role::DocumentText
                                    | Role::DocumentFrame
                                    | Role::DocumentWeb
                                    | Role::Terminal
                                    | Role::ComboBox
                                    | Role::SpinButton
                            )
                        );
                    if editable {
                        return Ok(Focus::TextField);
                    }
                    if !matches!(
                        role,
                        Some(Role::Frame | Role::Window | Role::Dialog | Role::Application)
                    ) {
                        saw_focused_non_text = true;
                    }
                }
                if let Ok(children) = p.get_children().await {
                    stack.extend(children);
                }
            }
        }
    }
    // Only claim "no text field" when we positively saw a focused widget
    // that is not editable inside an active window.
    Ok(if saw_active_window && saw_focused_non_text {
        Focus::NoTextField
    } else {
        Focus::Unknown
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_never_panics_headless() {
        // No a11y bus in CI → Unknown (or a real answer on a desktop).
        let _ = focused_text_field();
    }
}
