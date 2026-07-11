//! User-facing weather-effects toggle. The quality tier still gates whether
//! atmosphere can run at all (Medium/High only — see `QualityKnobs::
//! atmosphere_enabled`); this resource is the Settings checkbox that lets
//! players turn scrolling fog/clouds off even on those tiers.

use bevy_ecs::prelude::*;

/// Whether the player wants atmospheric weather (scrolling volumetric fog /
/// cloud) drawn when the active quality tier supports it.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherEffects {
    pub enabled: bool,
}

impl Default for WeatherEffects {
    fn default() -> Self {
        // On by default when the tier allows it; Potato/Low still skip the
        // effect via `QualityKnobs::atmosphere_enabled`.
        WeatherEffects { enabled: true }
    }
}
