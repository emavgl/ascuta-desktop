//! Dictation state machine: idle → recording → decoding → injecting → idle.
//! Pure logic, no OS calls — fully unit-tested and used by `--check`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Recording,
    Decoding,
    Injecting,
}

pub struct Core {
    phase: Phase,
}

impl Core {
    pub fn new() -> Self {
        Self { phase: Phase::Idle }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Push-to-talk pressed. Returns the new phase.
    pub fn on_hotkey_pressed(&mut self) -> Phase {
        if self.phase == Phase::Idle {
            self.phase = Phase::Recording;
        }
        self.phase
    }

    /// Push-to-talk released. Recording → Decoding (VAD/no-speech gate
    /// happens in M1; M0 always proceeds).
    pub fn on_hotkey_released(&mut self) -> Phase {
        if self.phase == Phase::Recording {
            self.phase = Phase::Decoding;
        }
        self.phase
    }

    /// STT finished. Empty transcript aborts back to idle (no paste).
    pub fn on_decoded(&mut self, has_text: bool) -> Phase {
        if self.phase == Phase::Decoding {
            self.phase = if has_text {
                Phase::Injecting
            } else {
                Phase::Idle
            };
        }
        self.phase
    }

    pub fn on_injected(&mut self) -> Phase {
        if self.phase == Phase::Injecting {
            self.phase = Phase::Idle;
        }
        self.phase
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path() {
        let mut c = Core::new();
        assert_eq!(c.on_hotkey_pressed(), Phase::Recording);
        assert_eq!(c.on_hotkey_released(), Phase::Decoding);
        assert_eq!(c.on_decoded(true), Phase::Injecting);
        assert_eq!(c.on_injected(), Phase::Idle);
    }

    #[test]
    fn empty_transcript_aborts_without_inject() {
        let mut c = Core::new();
        c.on_hotkey_pressed();
        c.on_hotkey_released();
        assert_eq!(c.on_decoded(false), Phase::Idle);
    }

    #[test]
    fn stray_events_are_ignored() {
        let mut c = Core::new();
        assert_eq!(c.on_hotkey_released(), Phase::Idle);
        assert_eq!(c.on_injected(), Phase::Idle);
    }
}
