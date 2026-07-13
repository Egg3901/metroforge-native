//! Weather -> gameplay coupling (v0.7).
//!
//! Port of `sim/src/core/weatherEffects.ts`. ALL tunable constants live here so
//! balance passes touch one file. Pure and deterministic: each effect is a
//! function of the current [`WeatherSnapshot`] only. Every multiplier is written
//! for a FULL-STRENGTH state and eased toward 1.0 (no effect) as intensity -> 0
//! via [`lerp_by_intensity`], so a drizzle barely matters and a downpour bites.

use crate::types::TransitMode;
use crate::weather::{WeatherEvent, WeatherSnapshot, WeatherState};

/// Ease a full-strength multiplier toward 1.0 (no effect) as intensity -> 0.
fn lerp_by_intensity(full_mult: f64, intensity: f64) -> f64 {
    let t = intensity.clamp(0.0, 1.0);
    1.0 + (full_mult - 1.0) * t
}

// -- Ridership ----------------------------------------------------------------
/// Total travel-demand multiplier at full strength.
pub fn weather_demand_mult_full(state: WeatherState) -> f64 {
    match state {
        WeatherState::Clear => 1.0,
        WeatherState::Overcast => 0.99,
        WeatherState::Rain => 0.93,
        WeatherState::Fog => 0.98,
        WeatherState::Snow => 0.82,
        WeatherState::Storm => 0.72,
    }
}

/// Walk-catchment multiplier at full strength (how far people walk to a stop).
pub fn weather_walk_mult_full(state: WeatherState) -> f64 {
    match state {
        WeatherState::Clear => 1.0,
        WeatherState::Overcast => 1.0,
        WeatherState::Rain => 0.85,
        WeatherState::Fog => 0.95,
        WeatherState::Snow => 0.75,
        WeatherState::Storm => 0.7,
    }
}

/// Extra generalized-cost MINUTES added to a car trip at full strength.
pub fn weather_car_penalty_min_full(state: WeatherState) -> f64 {
    match state {
        WeatherState::Clear | WeatherState::Overcast => 0.0,
        WeatherState::Rain => 6.0,
        WeatherState::Fog => 4.0,
        WeatherState::Snow => 12.0,
        WeatherState::Storm => 15.0,
    }
}

/// Extra demand multiplier while a blizzard event is active.
pub const BLIZZARD_DEMAND_MULT: f64 = 0.6;
/// Extra demand multiplier while a heatwave event is active.
pub const HEATWAVE_DEMAND_MULT: f64 = 0.9;

/// Total travel-demand multiplier under the current sky.
pub fn weather_demand_mult(weather: Option<&WeatherSnapshot>) -> f64 {
    let Some(w) = weather else { return 1.0 };
    let mut m = lerp_by_intensity(weather_demand_mult_full(w.state), w.intensity);
    match w.event {
        Some(WeatherEvent::Blizzard) => m *= BLIZZARD_DEMAND_MULT,
        Some(WeatherEvent::Heatwave) => m *= HEATWAVE_DEMAND_MULT,
        None => {}
    }
    m
}

/// Walk-catchment multiplier under the current sky.
pub fn weather_walk_mult(weather: Option<&WeatherSnapshot>) -> f64 {
    match weather {
        Some(w) => lerp_by_intensity(weather_walk_mult_full(w.state), w.intensity),
        None => 1.0,
    }
}

/// Car generalized-cost penalty (minutes) under the current sky.
pub fn weather_car_penalty_min(weather: Option<&WeatherSnapshot>) -> f64 {
    match weather {
        Some(w) => weather_car_penalty_min_full(w.state) * w.intensity.clamp(0.0, 1.0),
        None => 0.0,
    }
}

// -- Vehicle speed ------------------------------------------------------------
/// Surface-running speed multiplier at full strength.
pub fn weather_speed_mult_full(state: WeatherState) -> f64 {
    match state {
        WeatherState::Clear | WeatherState::Overcast => 1.0,
        WeatherState::Rain => 0.9,
        WeatherState::Fog => 0.92,
        WeatherState::Snow => 0.75,
        WeatherState::Storm => 0.7,
    }
}

/// Surface speed multiplier during a blizzard (scaled by surface exposure).
pub const BLIZZARD_SURFACE_SPEED_MULT: f64 = 0.35;
/// Heat-wave rail speed restriction (metro + rail).
pub const HEATWAVE_RAIL_SPEED_MULT: f64 = 0.9;

/// Speed multiplier for a route this tick. `surface_exposure` is the fraction of
/// the route NOT in tunnel; penalties scale with it so grade separation buys
/// immunity.
pub fn weather_speed_mult(
    weather: Option<&WeatherSnapshot>,
    mode: TransitMode,
    surface_exposure: f64,
) -> f64 {
    let Some(w) = weather else { return 1.0 };
    let exposure = surface_exposure.clamp(0.0, 1.0);
    let full = weather_speed_mult_full(w.state);
    let mut mult = 1.0 + (full - 1.0) * w.intensity.clamp(0.0, 1.0) * exposure;
    if w.event == Some(WeatherEvent::Blizzard) {
        mult *= 1.0 + (BLIZZARD_SURFACE_SPEED_MULT - 1.0) * exposure;
    }
    if w.event == Some(WeatherEvent::Heatwave)
        && (mode == TransitMode::Metro || mode == TransitMode::Rail)
    {
        mult *= HEATWAVE_RAIL_SPEED_MULT;
    }
    mult
}

// -- Construction -------------------------------------------------------------
/// Build-cost surcharge (added fraction) while a state is active.
pub fn weather_build_surcharge(state: WeatherState) -> f64 {
    match state {
        WeatherState::Clear | WeatherState::Overcast => 0.0,
        WeatherState::Rain => 0.08,
        WeatherState::Fog => 0.03,
        WeatherState::Snow => 0.2,
        WeatherState::Storm => 0.3,
    }
}

/// Build-cost multiplier (>= 1) for track laid under the current sky.
pub fn weather_build_cost_mult(weather: Option<&WeatherSnapshot>) -> f64 {
    match weather {
        Some(w) => 1.0 + weather_build_surcharge(w.state) * w.intensity.clamp(0.0, 1.0),
        None => 1.0,
    }
}

/// Build-TIME surcharge (added fraction). Defined for the future build-queue.
pub fn weather_build_time_surcharge(state: WeatherState) -> f64 {
    match state {
        WeatherState::Clear | WeatherState::Overcast => 0.0,
        WeatherState::Rain => 0.1,
        WeatherState::Fog => 0.05,
        WeatherState::Snow => 0.3,
        WeatherState::Storm => 0.5,
    }
}

// -- Reliability (hook for v0.9 ops sim) --------------------------------------
/// Base per-vehicle-per-day breakdown probability by weather, at full strength.
pub fn weather_breakdown_chance_full(state: WeatherState) -> f64 {
    match state {
        WeatherState::Clear | WeatherState::Overcast => 0.002,
        WeatherState::Rain => 0.006,
        WeatherState::Fog => 0.004,
        WeatherState::Snow => 0.02,
        WeatherState::Storm => 0.03,
    }
}

/// Per-vehicle-per-day breakdown chance for a mode under the current sky.
pub fn weather_breakdown_chance(weather: Option<&WeatherSnapshot>, _mode: TransitMode) -> f64 {
    match weather {
        None => weather_breakdown_chance_full(WeatherState::Clear),
        Some(w) => {
            weather_breakdown_chance_full(w.state) * (0.5 + 0.5 * w.intensity.clamp(0.0, 1.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weather::Season;

    fn snap(state: WeatherState, intensity: f64, event: Option<WeatherEvent>) -> WeatherSnapshot {
        WeatherSnapshot {
            state,
            intensity,
            season: Season::Winter,
            month: 0,
            event,
        }
    }

    #[test]
    fn no_weather_is_neutral() {
        assert_eq!(weather_demand_mult(None), 1.0);
        assert_eq!(weather_walk_mult(None), 1.0);
        assert_eq!(weather_speed_mult(None, TransitMode::Bus, 1.0), 1.0);
        assert_eq!(weather_build_cost_mult(None), 1.0);
    }

    #[test]
    fn storm_suppresses_demand() {
        let s = snap(WeatherState::Storm, 1.0, None);
        assert!(weather_demand_mult(Some(&s)) < 0.8);
    }

    #[test]
    fn underground_route_immune_to_surface_penalty() {
        let s = snap(WeatherState::Snow, 1.0, None);
        let surface = weather_speed_mult(Some(&s), TransitMode::Metro, 1.0);
        let tunnel = weather_speed_mult(Some(&s), TransitMode::Metro, 0.0);
        assert!(surface < tunnel);
        assert!((tunnel - 1.0).abs() < 1e-9);
    }

    #[test]
    fn heatwave_slows_rail_regardless_of_grade() {
        let s = snap(WeatherState::Clear, 0.8, Some(WeatherEvent::Heatwave));
        let rail = weather_speed_mult(Some(&s), TransitMode::Rail, 0.0);
        assert!((rail - HEATWAVE_RAIL_SPEED_MULT).abs() < 1e-9);
        let bus = weather_speed_mult(Some(&s), TransitMode::Bus, 0.0);
        assert!((bus - 1.0).abs() < 1e-9);
    }
}
