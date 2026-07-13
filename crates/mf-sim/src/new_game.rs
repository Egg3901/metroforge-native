//! Fresh-game assembly. Port of `sim/src/core/newGame.ts`.
//!
//! Builds a complete initial [`GameState`] from a seed + preset: runs procedural
//! worldgen ([`crate::city::generate_city`]), seeds the primary RNG stream,
//! aggregates population/jobs from the generated districts, and seeds the v0.9
//! ops sub-state so operations run deterministically from tick 0. This is the
//! state P3's `sim_tick` will advance.
//!
//! Deferred to P3 (stubbed with TODOs, matching the transient-field contract):
//! * `weather = weatherAt(...)` (weather.ts) — left `None`; transient, unhashed.
//! * scenario rule/event derivation from a `ScenarioDef` (scenario/evaluate.ts).
//! * OSM real-city bundles (`options.osm`) — P2 is procedural-only.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::city::generate_city;
use crate::city::presets::{preset_by_key, MapSize};
use crate::constants::starting_cash;
use crate::rng::Rng;
use crate::types::{
    Budget, CityStats, DayLedger, Difficulty, GameState, Period, ScenarioRules, TransitMode,
};

/// Options for [`new_game`]. Mirrors `NewGameOptions` (procedural subset).
#[derive(Clone, Debug, Default)]
pub struct NewGameOptions {
    /// Map size (world edge length). `None` = the preset/default world.
    pub size: Option<MapSize>,
    /// City preset key (`"nyc"`, `"boston"`, ...). `None` = generic.
    pub preset_key: Option<String>,
    /// Era / challenge constraints applied at kickoff.
    pub rules: Option<ScenarioRules>,
    /// Preloaded real-city OSM bundle. When `Some`, real land/water/roads +
    /// baked masks/elevation/labels/anchors replace procgen. The host layer
    /// (`mf-net`) resolves + parses this from the city key. `None` = procedural.
    pub osm: Option<crate::city::osm::OsmCityData>,
    // TODO(P5): `scenario` ScenarioDef.
}

/// Transient per-process instance-id counter. Mirrors `nextInstanceId()`
/// (instance.ts). Not hashed, not serialized.
static INSTANCE_COUNTER: AtomicU32 = AtomicU32::new(1);

fn next_instance_id() -> u32 {
    INSTANCE_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// Which service period tick 0 falls in. Tick 0 = midnight -> Night. Mirrors
/// `periodForTick(0)` (ops/periods.ts). (Full port lands with ops in P3.)
fn period_for_tick_zero() -> Period {
    Period::Night
}

/// Assemble a fresh [`GameState`]. Mirrors `newGame`.
pub fn new_game(seed: u32, difficulty: Difficulty, options: NewGameOptions) -> GameState {
    let preset = preset_by_key(options.preset_key.as_deref());
    let world_size = options.size.map(|s| s.meters());
    let city = generate_city(seed, difficulty, world_size, preset, options.osm.as_ref());

    // secondary stream seeded exactly like the TS newGame (never advanced here)
    let rng = Rng::from_seed(seed ^ 0x5bd1_e995);

    let mut population = 0.0;
    let mut jobs = 0.0;
    for d in &city.districts {
        population += d.population;
        jobs += d.jobs;
    }

    let rules = options.rules.clone();
    let starting_modes: Vec<TransitMode> = match &rules {
        Some(r) if !r.starting_modes.is_empty() => r.starting_modes.clone(),
        _ => vec![TransitMode::Bus],
    };
    let cash = rules
        .as_ref()
        .and_then(|r| r.starting_cash)
        .unwrap_or_else(|| starting_cash(difficulty));

    // v0.9 System A: seed the ops sub-state on a dedicated, independent stream so
    // breakdown rolls never reorder the events/growth stream. Mirrors `initOps`
    // on a fresh game (no routes yet, so the route reconcile loop is a no-op).
    let ops_rng_state = Rng::from_seed(seed ^ 0x09ab_5eed).state();

    GameState {
        seed,
        tick: 0,
        rng_state: rng.state(),
        difficulty,
        city_key: options.preset_key.clone(),
        fields: city.fields,
        roads: city.roads,
        districts: city.districts,
        stations: Vec::new(),
        tracks: Vec::new(),
        routes: Vec::new(),
        vehicles: Vec::new(),
        flows: Vec::new(),
        budget: Budget {
            cash,
            loan_balance: 0.0,
            loan_rate: 0.08,
            last_day: DayLedger::default(),
            net_history: Vec::new(),
            lifetime: None,
        },
        stats: CityStats {
            population,
            jobs,
            daily_transit_trips: 0.0,
            daily_car_trips: 0.0,
            transit_share: 0.0,
            coverage: 0.0,
            approval: 50.0,
        },
        next_id: 1,
        demand_dirty: true,
        unlocked_modes: starting_modes,
        active_events: Vec::new(),
        next_event_day: 8, // no events in the first week
        scenario_rules: rules,
        scenario: None,
        scenario_won: None,
        fired_scenario_events: None,
        district_demand_mult: None,
        global_demand_mult: None,
        global_demand_mult_days_left: None,
        command_log: Vec::new(),
        low_approval_days: 0,
        bankrupt_days: 0,
        failed: None,

        // ── v0.9 ops sub-state (initOps) ──
        fleet: Some(Vec::new()),
        depots: Some(Vec::new()),
        incidents: Some(Vec::new()),
        ops_rng_state: Some(ops_rng_state),
        ops_period: Some(period_for_tick_zero()),
        ops_daily: Some(BTreeMap::new()),

        // ── transient ──
        instance_id: next_instance_id(),
        // TODO(P3): weather = weatherAt(seed, 0, climateTable(cityKey)).
        weather: None,
        last_weather_event: None,
        traffic: None,
        unserved: None,
        analytics: None,
        // real-city static channels (None for procedural cities)
        osm_water_mask: city.water_mask_hi,
        osm_park_mask: city.park_mask_hi,
        osm_building_mask: city.building_mask_hi,
        osm_mask_res: city.mask_res,
        osm_elevation: city.elevation_hi,
        osm_elev_res: city.elev_res,
        osm_labels: if city.labels.is_empty() {
            None
        } else {
            Some(city.labels)
        },
        poi_anchors: if city.poi_anchors.is_empty() {
            None
        } else {
            Some(city.poi_anchors)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_game_is_populated() {
        let s = new_game(12345, Difficulty::Normal, NewGameOptions::default());
        assert_eq!(s.tick, 0);
        assert_eq!(s.fields.w, 96);
        assert!(!s.roads.is_empty(), "roads generated");
        assert!(!s.districts.is_empty(), "districts generated");
        assert!(s.stats.population > 0.0, "population aggregated");
        assert!(s.districts.iter().all(|d| !d.name.is_empty()), "named");
        assert!(s.fleet.is_some() && s.ops_rng_state.is_some(), "ops seeded");
    }
}
