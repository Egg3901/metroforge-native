//! Save format + the deterministic state fingerprint. Port of
//! `sim/src/core/save.ts`.
//!
//! # `state_hash` — the determinism contract
//!
//! [`state_hash`] mirrors `save.ts::stateHash` (line 186) EXACTLY in which
//! fields it feeds and in what order. This is the one place where matching the
//! TS behavior matters: the SET and ORDER of hashed fields must follow the TS
//! source so the Rust determinism baseline is meaningful. The hash ALGORITHM is
//! our own choice (FNV-1a via [`StateHasher`], per PORT.md), not the TS
//! `Math.imul` mixer, so the numeric hash values differ from JS by design.
//!
//! Like the TS `mix()`, each numeric field is rounded to micro-units
//! (`round(v * 1000)`) before hashing, so a JSON save round-trip (which can
//! perturb the last f64 bits) does not change the fingerprint.
//!
//! ## Audited hashed field set + order (from save.ts::stateHash)
//!
//! 1. `tick`
//! 2. `budget.cash`
//! 3. `stats.population`
//! 4. `stations.len()`
//! 5. `tracks.len()`
//! 6. `routes.len()`
//! 7. for each route (in order): `daily_ridership`, `vehicle_count`,
//!    `on_time_pct` (defaulting to `1.0` when unset)
//! 8. for each vehicle (in order): `along`
//! 9. for each fleet unit (in order): `condition`, `status` (0 = active,
//!    1 = maintenance, 2 = broken down)
//! 10. `incidents.len()` (0 when the ops sub-state is absent)
//!
//! Everything else in `GameState` — seed, RNG streams, roads, districts,
//! fields, the whole transient region — is deliberately NOT hashed, matching
//! the TS source.

use crate::hash::StateHasher;
use crate::types::{FleetStatus, GameState};

/// Feed one numeric field the way `save.ts::mix` does: round to micro-units and
/// mix the resulting integer. Keeps the fingerprint stable across JSON
/// round-trips (which is exactly why the TS source rounds).
#[inline]
fn mix(h: &mut StateHasher, v: f64) {
    h.write_i64((v * 1000.0).round() as i64);
}

/// Deterministic state fingerprint. See the module docs for the audited field
/// set + order this mirrors from `save.ts::stateHash`.
pub fn state_hash(state: &GameState) -> u64 {
    let mut h = StateHasher::new();
    mix(&mut h, state.tick as f64);
    mix(&mut h, state.budget.cash);
    mix(&mut h, state.stats.population);
    mix(&mut h, state.stations.len() as f64);
    mix(&mut h, state.tracks.len() as f64);
    mix(&mut h, state.routes.len() as f64);
    for r in &state.routes {
        mix(&mut h, r.daily_ridership);
        mix(&mut h, r.vehicle_count as f64);
        // v0.9 ops: reliability is part of the deterministic state; unset => 1.
        mix(&mut h, r.on_time_pct.unwrap_or(1.0));
    }
    for v in &state.vehicles {
        mix(&mut h, v.along);
    }
    // v0.9 ops: fleet condition + status.
    if let Some(fleet) = &state.fleet {
        for u in fleet {
            mix(&mut h, u.condition);
            let status = match u.status {
                FleetStatus::Active => 0.0,
                FleetStatus::Maintenance => 1.0,
                FleetStatus::BrokenDown => 2.0,
            };
            mix(&mut h, status);
        }
    }
    let incident_count = state.incidents.as_ref().map_or(0, |i| i.len());
    mix(&mut h, incident_count as f64);
    h.finish()
}

/// Current save format version. Mirrors `SAVE_VERSION` (save.ts).
pub const SAVE_VERSION: u32 = 3;

/// Serialize the game state to JSON. Transient fields carry `#[serde(skip)]`
/// (see [`GameState`]), so this mirrors `save.ts::serialize`'s exclusion set.
///
/// NOTE: unlike the TS serializer this does not (yet) collapse polylines to
/// points-only; the Rust save stores the cumulative lengths inline. Both round
/// trip losslessly; the compact-polyline optimization is a P2/P4 concern.
#[cfg(feature = "serde")]
pub fn serialize(state: &GameState) -> Result<String, serde_json::Error> {
    serde_json::to_string(state)
}

/// Deserialize a game state from JSON produced by [`serialize`]. Transient
/// fields are reconstructed to their defaults (`None` / `0`), matching the TS
/// "recomputed on load" contract; recomputing weather / ops sub-state lands
/// with those systems (P3).
#[cfg(feature = "serde")]
pub fn deserialize(json: &str) -> Result<GameState, serde_json::Error> {
    serde_json::from_str(json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Difficulty, FleetUnit, TransitMode, VehicleState};

    fn base() -> GameState {
        GameState::with_difficulty(123, Difficulty::Normal)
    }

    #[test]
    fn hash_is_stable_across_two_calls() {
        let s = base();
        assert_eq!(state_hash(&s), state_hash(&s));
    }

    #[test]
    fn hashed_field_change_changes_hash() {
        let mut s = base();
        let before = state_hash(&s);
        s.budget.cash += 1.0;
        assert_ne!(before, state_hash(&s));
    }

    #[test]
    fn transient_field_change_leaves_hash_unchanged() {
        let mut s = base();
        let before = state_hash(&s);
        // seed is NOT hashed by save.ts::stateHash; nor is the RNG stream, nor
        // the whole transient region.
        s.instance_id = 999;
        // weather is transient (real snapshot from the weather system).
        s.weather = Some(crate::weather::weather_at(
            s.seed,
            s.tick,
            &crate::weather::climate_table(None),
        ));
        s.bankrupt_days = 42;
        s.rng_state = [1, 2, 3, 4];
        assert_eq!(before, state_hash(&s));
    }

    #[test]
    fn fleet_and_vehicle_fields_enter_hash() {
        let mut s = base();
        let before = state_hash(&s);
        s.vehicles.push(VehicleState {
            id: 1,
            route_id: 1,
            along: 5.0,
            path_length: 10.0,
            dwell_remaining: 0.0,
            occupancy: 0.0,
        });
        let after_vehicle = state_hash(&s);
        assert_ne!(before, after_vehicle);
        s.fleet = Some(vec![FleetUnit {
            id: 1,
            mode: TransitMode::Bus,
            route_id: None,
            age_days: 0.0,
            condition: 0.9,
            status: FleetStatus::Active,
            status_ticks_left: 0,
        }]);
        assert_ne!(after_vehicle, state_hash(&s));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_roundtrip_preserves_hash() {
        let s = base();
        let json = serialize(&s).unwrap();
        let back = deserialize(&json).unwrap();
        assert_eq!(state_hash(&s), state_hash(&back));
    }
}
