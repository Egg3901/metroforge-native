//! User-facing day/night toggle. The quality tier still gates whether the
//! day/night cycle animates at all (Potato pins noon — see
//! `QualityKnobs::day_night_enabled`); this resource is the Settings checkbox
//! that lets players pin a flat daytime wash even on tiers that support the
//! cycle. `mf-render`'s `daynight.rs` reads it; `mf-game` keeps it synced from
//! `config.toml`.

use bevy_ecs::prelude::*;

/// Whether the player wants the animated day/night cycle. When `false`, the
/// sky/light hold at noon regardless of the sim clock (on tiers that would
/// otherwise animate it).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayNightEnabled {
    /// When `true`, the day/night cycle animates (on tiers that support it);
    /// when `false`, the sky/light hold at noon.
    pub enabled: bool,
}

impl Default for DayNightEnabled {
    fn default() -> Self {
        DayNightEnabled { enabled: true }
    }
}
