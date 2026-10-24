//! Display modes and the rules that choose between them.

use chrono::Duration;

use crate::sources::RadarStatus;

/// Consecutive radar-status samples that must agree before the mode switches.
/// A station that alternates Operate/Standby on successive polls therefore
/// cannot force a new render every minute.
pub const MODE_SWITCH_SAMPLES: u32 = 3;

/// The current mode is held for at least this long before a switch is
/// accepted, even when the required samples already agree.
pub const MODE_MIN_DWELL: Duration = Duration::minutes(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Storm,
    Clear,
    /// Photographs from the `photos/` directory. Never selected from the radar
    /// status; only an explicit manual selection can enter it.
    Photo,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Storm => "Storm",
            Mode::Clear => "Clear",
            Mode::Photo => "Photo",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Storm" => Some(Mode::Storm),
            "Clear" => Some(Mode::Clear),
            "Photo" => Some(Mode::Photo),
            _ => None,
        }
    }

    /// Storm every 10 minutes, clear every 30, photos every 10.
    pub fn interval(self) -> Duration {
        match self {
            Mode::Storm => Duration::minutes(10),
            Mode::Clear => Duration::minutes(30),
            Mode::Photo => Duration::minutes(10),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModeSelect {
    Auto,
    Manual,
}

impl ModeSelect {
    pub fn name(self) -> &'static str {
        match self {
            ModeSelect::Auto => "Auto",
            ModeSelect::Manual => "Manual",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "Auto" => Some(ModeSelect::Auto),
            "Manual" => Some(ModeSelect::Manual),
            _ => None,
        }
    }
}

/// The mode the station's status calls for.
pub fn mode_for(status: RadarStatus) -> Mode {
    if status.up && status.vcp != -1 && !status.clear_air_mode() {
        Mode::Storm
    } else {
        Mode::Clear
    }
}
