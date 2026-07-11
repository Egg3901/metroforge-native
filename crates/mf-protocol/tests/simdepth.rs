//! Deserialization tests for the sim-depth fields the TS sidecar started
//! emitting in metroforge PR #31 (`hourOfDay`, `demandFactor`,
//! `fareboxRecovery`, `lifetime`, `districts[]`, `overcrowdedRoutes[]`, and
//! per-route `liveCrowding`/`operatingCost`/`farebox`). All of them are
//! `serde(default)` so an OLD sidecar payload that omits them still parses;
//! these tests pin both the old-payload (fields absent) and new-payload
//! (fields present) shapes.

use mf_protocol::{UiRoute, UiState};

/// A `UiState` JSON object with every REQUIRED field but NONE of the
/// sim-depth extras — i.e. exactly what an older sidecar sends.
fn legacy_ui_state_json() -> String {
    r##"{
        "tick": 42,
        "insights": ["Ridership is climbing."],
        "day": 3,
        "speed": 10.0,
        "cash": 12345.0,
        "loanBalance": 0.0,
        "lastDay": {"fares": 100.0, "subsidy": 50.0, "operations": 30.0, "maintenance": 10.0, "interest": 5.0},
        "netHistory": [10.0, 20.0, 30.0],
        "population": 100000.0,
        "approval": 55.0,
        "transitShare": 0.3,
        "coverage": 0.4,
        "dailyTransitTrips": 5000.0,
        "unlockedModes": ["bus", "tram"],
        "stations": [],
        "tracks": [],
        "routes": [{
            "id": 1,
            "name": "Red Line",
            "color": "#ff0000",
            "mode": "bus",
            "stationIds": [10, 20],
            "headwaySeconds": 300.0,
            "fare": 2.0,
            "vehicleCount": 3,
            "dailyRidership": 1000.0,
            "dailyRevenue": 2000.0,
            "lengthMeters": 1500.0,
            "capacity": 400.0,
            "load": 0.6,
            "crowding": 0.5,
            "segmentLoads": [0.3, 0.7]
        }],
        "activeEvents": [],
        "fieldsVersion": 1,
        "bankrupt": false,
        "commandCount": 7
    }"##
    .to_string()
}

#[test]
fn legacy_payload_without_sim_depth_still_parses() {
    let state: UiState =
        serde_json::from_str(&legacy_ui_state_json()).expect("legacy payload must deserialize");

    // Every new field defaults cleanly on an old payload.
    assert_eq!(state.hour_of_day, None);
    assert_eq!(state.demand_factor, None);
    assert_eq!(state.farebox_recovery, None);
    assert_eq!(state.lifetime, None);
    assert!(state.districts.is_empty());
    assert!(state.overcrowded_routes.is_none());

    let route = &state.routes[0];
    assert_eq!(route.live_crowding, None);
    assert_eq!(route.operating_cost, None);
    assert_eq!(route.farebox, None);

    // With no `hourOfDay`, the display clock falls back to the tick clock:
    // tick 42 of 1200 ticks/day == 0.84 hours.
    assert!((state.display_hour() - 0.84).abs() < 1e-6);
}

#[test]
fn new_payload_with_sim_depth_parses_every_field() {
    // Take the legacy object and splice the new fields in.
    let json = r##"{
        "tick": 600,
        "insights": [],
        "day": 3,
        "speed": 10.0,
        "cash": 12345.0,
        "loanBalance": 0.0,
        "lastDay": {"fares": 100.0, "subsidy": 50.0, "operations": 30.0, "maintenance": 10.0, "interest": 5.0},
        "netHistory": [],
        "population": 100000.0,
        "approval": 55.0,
        "transitShare": 0.3,
        "coverage": 0.4,
        "dailyTransitTrips": 5000.0,
        "unlockedModes": ["bus"],
        "stations": [],
        "tracks": [],
        "routes": [{
            "id": 1,
            "name": "Red Line",
            "color": "#ff0000",
            "mode": "bus",
            "stationIds": [10, 20],
            "headwaySeconds": 300.0,
            "fare": 2.0,
            "vehicleCount": 3,
            "dailyRidership": 1000.0,
            "dailyRevenue": 2000.0,
            "lengthMeters": 1500.0,
            "capacity": 400.0,
            "load": 0.6,
            "crowding": 0.5,
            "segmentLoads": [0.3, 0.7],
            "liveCrowding": 0.92,
            "operatingCost": 1400.0,
            "farebox": 2000.0
        }],
        "activeEvents": [],
        "fieldsVersion": 1,
        "bankrupt": false,
        "commandCount": 7,
        "hourOfDay": 8.5,
        "demandFactor": 1.7,
        "fareboxRecovery": 1.43,
        "lifetime": {
            "fares": 500000.0,
            "subsidy": 400000.0,
            "operations": 300000.0,
            "maintenance": 100000.0,
            "interest": 50000.0,
            "days": 120.0
        },
        "districts": [
            {"id": 1, "name": "Downtown", "x": 100.0, "y": 200.0, "population": 40000.0, "jobs": 55000.0},
            {"id": 2, "name": "Riverside", "x": -50.0, "y": 30.0, "population": 12000.0, "jobs": 3000.0}
        ],
        "overcrowdedRoutes": 2
    }"##;

    let state: UiState = serde_json::from_str(json).expect("new payload must deserialize");

    assert_eq!(state.hour_of_day, Some(8.5));
    assert_eq!(state.demand_factor, Some(1.7));
    assert_eq!(state.farebox_recovery, Some(1.43));
    let lifetime = state.lifetime.as_ref().expect("lifetime ledger present");
    assert!((lifetime.fares - 500000.0).abs() < 1e-6);
    assert!((lifetime.days - 120.0).abs() < 1e-6);
    assert_eq!(state.overcrowded_routes, Some(2));
    assert_eq!(state.districts.len(), 2);
    assert_eq!(state.districts[0].name, "Downtown");
    assert!((state.districts[1].jobs - 3000.0).abs() < 1e-6);

    let route = &state.routes[0];
    assert_eq!(route.live_crowding, Some(0.92));
    assert_eq!(route.operating_cost, Some(1400.0));
    assert_eq!(route.farebox, Some(2000.0));

    // Sim `hourOfDay` wins over the tick-derived clock.
    assert!((state.display_hour() - 8.5).abs() < 1e-6);
}

#[test]
fn display_hour_wraps_out_of_range_sim_hours() {
    let json = r##"{
        "tick": 0, "insights": [], "day": 1, "speed": 1.0, "cash": 0.0, "loanBalance": 0.0,
        "lastDay": {"fares": 0.0, "subsidy": 0.0, "operations": 0.0, "maintenance": 0.0, "interest": 0.0},
        "netHistory": [], "population": 0.0, "approval": 50.0, "transitShare": 0.0, "coverage": 0.0,
        "dailyTransitTrips": 0.0, "unlockedModes": [], "stations": [], "tracks": [], "routes": [],
        "activeEvents": [], "fieldsVersion": 1, "bankrupt": false, "commandCount": 0,
        "hourOfDay": 25.5
    }"##;
    let state: UiState = serde_json::from_str(json).expect("must deserialize");
    assert!((state.display_hour() - 1.5).abs() < 1e-6);
}

#[test]
fn route_missing_only_some_sim_depth_fields_defaults_the_rest() {
    // A route that carries liveCrowding but not the cost pair (defensive:
    // the client must not assume the three per-route extras arrive together).
    let json = r##"{
        "id": 9, "name": "", "color": "#000000", "mode": "tram", "stationIds": [1, 2],
        "headwaySeconds": 200.0, "fare": 1.5, "vehicleCount": 1, "dailyRidership": 0.0,
        "dailyRevenue": 0.0, "lengthMeters": 0.0, "capacity": 0.0, "load": 0.0,
        "crowding": 0.0, "segmentLoads": [], "liveCrowding": 0.4
    }"##;
    let route: UiRoute = serde_json::from_str(json).expect("partial route must deserialize");
    assert_eq!(route.live_crowding, Some(0.4));
    assert_eq!(route.operating_cost, None);
    assert_eq!(route.farebox, None);
}
