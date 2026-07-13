//! Roundtrip / back-compat tests for the v0.9 System A (Operations) UiState +
//! UiRoute fields and the two new Commands (`setRouteFrequency`, `buildDepot`).
//! Every new field is `serde(default)` so a PRE-v0.9 sidecar payload that omits
//! them still parses; these tests pin both the absent (old) and present (new)
//! shapes plus the new command variants.

use mf_protocol::{Command, TransitMode, UiRoute, UiState, Vec2};

/// A minimal legacy `UiState` object with no ops fields at all.
fn legacy_ui_json() -> &'static str {
    r##"{
        "tick": 10, "insights": [], "day": 1, "speed": 1.0, "cash": 0.0, "loanBalance": 0.0,
        "lastDay": {"fares": 0.0, "subsidy": 0.0, "operations": 0.0, "maintenance": 0.0, "interest": 0.0},
        "netHistory": [], "population": 0.0, "approval": 50.0, "transitShare": 0.0, "coverage": 0.0,
        "dailyTransitTrips": 0.0, "unlockedModes": [], "stations": [], "tracks": [], "routes": [],
        "activeEvents": [], "fieldsVersion": 1, "bankrupt": false, "commandCount": 0
    }"##
}

#[test]
fn legacy_ui_without_ops_still_parses() {
    let s: UiState = serde_json::from_str(legacy_ui_json()).expect("legacy must parse");
    assert!(s.fleet.is_none());
    assert!(s.depots.is_empty());
    assert!(s.incidents.is_empty());
    assert!(s.service_period.is_none());
    assert!(s.service_period_label.is_none());
}

#[test]
fn ui_with_ops_parses_every_field() {
    let json = r##"{
        "tick": 700, "insights": [], "day": 2, "speed": 10.0, "cash": 5.0, "loanBalance": 0.0,
        "lastDay": {"fares": 0.0, "subsidy": 0.0, "operations": 0.0, "maintenance": 0.0, "interest": 0.0},
        "netHistory": [], "population": 0.0, "approval": 50.0, "transitShare": 0.0, "coverage": 0.0,
        "dailyTransitTrips": 0.0, "unlockedModes": ["bus"], "stations": [], "tracks": [],
        "routes": [{
            "id": 1, "name": "R", "color": "#fff", "mode": "bus", "stationIds": [1,2],
            "headwaySeconds": 300.0, "fare": 2.0, "vehicleCount": 4, "dailyRidership": 0.0,
            "dailyRevenue": 0.0, "lengthMeters": 0.0, "capacity": 0.0, "load": 0.0,
            "crowding": 0.0, "segmentLoads": [],
            "onTimePct": 0.82, "avgDelaySec": 45.0, "inServiceVehicles": 3,
            "peakUnitsRequired": 6, "frequency": {"amPeak": 300.0, "night": 1200.0}
        }],
        "activeEvents": [], "fieldsVersion": 1, "bankrupt": false, "commandCount": 0,
        "fleet": {"total": 6, "active": 4, "maintenance": 1, "brokenDown": 1, "avgCondition": 0.77, "avgAgeDays": 12.0},
        "depots": [{"id": 90, "mode": "bus", "x": 1.0, "y": 2.0}],
        "incidents": [{"id": 5, "routeId": 1, "ticksLeft": 30}],
        "servicePeriod": "amPeak", "servicePeriodLabel": "AM peak"
    }"##;
    let s: UiState = serde_json::from_str(json).expect("ops payload must parse");
    let fleet = s.fleet.expect("fleet present");
    assert_eq!(fleet.total, 6);
    assert_eq!(fleet.broken_down, 1);
    assert!((fleet.avg_condition - 0.77).abs() < 1e-9);
    assert_eq!(s.depots.len(), 1);
    assert_eq!(s.depots[0].mode, "bus");
    assert_eq!(s.incidents[0].ticks_left, 30);
    assert_eq!(s.service_period.as_deref(), Some("amPeak"));
    assert_eq!(s.service_period_label.as_deref(), Some("AM peak"));

    let r = &s.routes[0];
    assert_eq!(r.on_time_pct, Some(0.82));
    assert_eq!(r.avg_delay_sec, Some(45.0));
    assert_eq!(r.in_service_vehicles, Some(3));
    assert_eq!(r.peak_units_required, Some(6));
    let freq = r.frequency.as_ref().expect("frequency present");
    assert!((freq["amPeak"] - 300.0).abs() < 1e-9);
}

#[test]
fn route_ops_fields_default_when_absent() {
    // A route with none of the ops extras (older sidecar) must default them.
    let json = r##"{
        "id": 1, "name": "R", "color": "#fff", "mode": "tram", "stationIds": [1,2],
        "headwaySeconds": 200.0, "fare": 1.5, "vehicleCount": 2, "dailyRidership": 0.0,
        "dailyRevenue": 0.0, "lengthMeters": 0.0, "capacity": 0.0, "load": 0.0,
        "crowding": 0.0, "segmentLoads": []
    }"##;
    let r: UiRoute = serde_json::from_str(json).expect("route must parse");
    assert_eq!(r.on_time_pct, None);
    assert_eq!(r.in_service_vehicles, None);
    assert!(r.frequency.is_none());
}

#[test]
fn set_route_frequency_command_roundtrips() {
    let c = Command::SetRouteFrequency {
        route_id: 7,
        period: "pmPeak".to_string(),
        headway_seconds: 240.0,
    };
    let json = serde_json::to_string(&c).unwrap();
    assert!(json.contains("\"kind\":\"setRouteFrequency\""));
    assert!(json.contains("\"routeId\":7"));
    assert!(json.contains("\"headwaySeconds\":240.0"));
    let back: Command = serde_json::from_str(&json).unwrap();
    assert_eq!(c, back);
}

#[test]
fn build_depot_command_roundtrips() {
    let c = Command::BuildDepot {
        mode: TransitMode::Metro,
        pos: Vec2 { x: 3.0, y: -4.0 },
    };
    let json = serde_json::to_string(&c).unwrap();
    assert!(json.contains("\"kind\":\"buildDepot\""));
    assert!(json.contains("\"mode\":\"metro\""));
    let back: Command = serde_json::from_str(&json).unwrap();
    assert_eq!(c, back);
}
