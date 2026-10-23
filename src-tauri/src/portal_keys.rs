//! Keyboard injection on Wayland through the XDG **RemoteDesktop** portal.
//!
//! Wayland clients cannot synthesise key events for other windows, and the
//! X11 `XTEST` route (enigo) only reaches XWayland clients. The portal is the
//! sanctioned way: the compositor shows a one-time consent dialog, then the
//! app may send key events. We request a persistent restore token, stored in
//! the config dir, so the dialog appears only once.
//!
//! A dedicated thread owns the portal session; callers send commands over a
//! channel and wait (bounded) for the result.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use ashpd::desktop::remote_desktop::{
    DeviceType, KeyState, NotifyKeyboardKeysymOptions, RemoteDesktop, SelectDevicesOptions,
};
use ashpd::desktop::PersistMode;

const KEYSYM_SHIFT_L: i32 = 0xffe1;
const KEYSYM_INSERT: i32 = 0xff63;
const KEYSYM_RETURN: i32 = 0xff0d;

enum Cmd {
    Paste(mpsc::Sender<Result<(), String>>),
    Type(String, mpsc::Sender<Result<(), String>>),
    Enter(mpsc::Sender<Result<(), String>>),
}

/// X11 keysym for a character, as expected by `NotifyKeyboardKeysym`.
/// Printable ASCII maps to itself; everything else uses the Unicode keysym
/// range (`0x01000000 + codepoint`), which Mutter and KWin accept.
fn keysym_for(c: char) -> Option<i32> {
    match c {
        '\r' => None,
        '\n' => Some(KEYSYM_RETURN),
        '\t' => Some(0xff09),
        ' '..='~' => Some(c as i32),
        _ => {
            let cp = c as u32;
            i32::try_from(0x0100_0000u32 + cp).ok()
        }
    }
}

pub struct PortalKeyboard {
    tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
}

fn token_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ascuta")
        .join("portal-token")
}

impl PortalKeyboard {
    /// Connect to the portal. Blocks until the user answers the consent
    /// dialog (first run) or the saved token is accepted.
    pub fn connect() -> Result<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("ascuta-portal".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                rt.block_on(async move {
                    let setup = async {
                        let rd = RemoteDesktop::new().await?;
                        let session = rd.create_session(Default::default()).await?;
                        let token = std::fs::read_to_string(token_path()).ok();
                        let devices: ashpd::enumflags2::BitFlags<DeviceType> =
                            DeviceType::Keyboard.into();
                        rd.select_devices(
                            &session,
                            SelectDevicesOptions::default()
                                .set_devices(devices)
                                .set_persist_mode(PersistMode::ExplicitlyRevoked)
                                .set_restore_token(token.as_deref().map(str::trim)),
                        )
                        .await?
                        .response()?;
                        let started = rd.start(&session, None, Default::default()).await?;
                        let selected = started.response()?;
                        if let Some(t) = selected.restore_token() {
                            if let Some(dir) = token_path().parent() {
                                let _ = std::fs::create_dir_all(dir);
                            }
                            let _ = std::fs::write(token_path(), t);
                        }
                        anyhow::Ok((rd, session))
                    };
                    let (rd, session) = match setup.await {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = ready_tx.send(Err(format!("{e:#}")));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(()));

                    let key = |sym: i32, st: KeyState| {
                        rd.notify_keyboard_keysym(
                            &session,
                            sym,
                            st,
                            NotifyKeyboardKeysymOptions::default(),
                        )
                    };
                    while let Some(cmd) = rx.recv().await {
                        let (reply, res) = match cmd {
                            Cmd::Paste(reply) => {
                                let r = async {
                                    key(KEYSYM_SHIFT_L, KeyState::Pressed).await?;
                                    key(KEYSYM_INSERT, KeyState::Pressed).await?;
                                    key(KEYSYM_INSERT, KeyState::Released).await?;
                                    key(KEYSYM_SHIFT_L, KeyState::Released).await?;
                                    anyhow::Ok(())
                                }
                                .await;
                                (reply, r)
                            }
                            Cmd::Type(text, reply) => {
                                let r = async {
                                    for c in text.chars() {
                                        let Some(sym) = keysym_for(c) else {
                                            continue;
                                        };
                                        key(sym, KeyState::Pressed).await?;
                                        key(sym, KeyState::Released).await?;
                                    }
                                    anyhow::Ok(())
                                }
                                .await;
                                (reply, r)
                            }
                            Cmd::Enter(reply) => {
                                let r = async {
                                    key(KEYSYM_RETURN, KeyState::Pressed).await?;
                                    key(KEYSYM_RETURN, KeyState::Released).await?;
                                    anyhow::Ok(())
                                }
                                .await;
                                (reply, r)
                            }
                        };
                        let _ = reply.send(res.map_err(|e| format!("{e:#}")));
                    }
                });
            })?;
        // First run: the user has to answer the compositor dialog.
        match ready_rx.recv_timeout(Duration::from_secs(90)) {
            Ok(Ok(())) => Ok(Self { tx }),
            Ok(Err(e)) => Err(anyhow!("keyboard portal: {e}")),
            Err(_) => Err(anyhow!("keyboard portal: no answer from the desktop")),
        }
    }

    fn call(&self, make: impl FnOnce(mpsc::Sender<Result<(), String>>) -> Cmd) -> Result<()> {
        self.call_timeout(make, Duration::from_secs(5))
    }

    fn call_timeout(
        &self,
        make: impl FnOnce(mpsc::Sender<Result<(), String>>) -> Cmd,
        timeout: Duration,
    ) -> Result<()> {
        let (rtx, rrx) = mpsc::channel();
        self.tx
            .send(make(rtx))
            .map_err(|_| anyhow!("keyboard portal thread stopped"))?;
        rrx.recv_timeout(timeout)
            .map_err(|_| anyhow!("keyboard portal timed out"))?
            .map_err(|e| anyhow!(e))
    }

    pub fn paste(&self) -> Result<()> {
        self.call(Cmd::Paste)
    }

    pub fn type_text(&self, text: &str) -> Result<()> {
        // One D-Bus round trip per character: allow more time for long text.
        self.call_timeout(
            |reply| Cmd::Type(text.to_string(), reply),
            Duration::from_secs(60),
        )
    }

    pub fn enter(&self) -> Result<()> {
        self.call(Cmd::Enter)
    }
}
