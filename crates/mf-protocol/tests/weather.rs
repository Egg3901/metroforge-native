//! Weather fields (v0.7) on `UiState`. All four are `serde(default)` so a
//! sidecar that predates weather still decodes; these tests pin both the
//! old-payload (fields absent) and new-payload (fields present) shapes so an
//! old sidecar and the render lane's future client agree.

use mf_protocol::{Season, UiState, WeatherEvent, WeatherState};

/// A `UiState` with every REQUIRED field but no weather — a pre-v0.7 sidecar.
fn legacy_ui_state_json() -> &'static str {
    r##"{
        "tick": 42, "insights": [], "day": 3, "speed": 10.0, "cash": 12345.0,
        "loanBalance": 0.0,
        "lastDay": {"fares": 100.0, "subsidy": 50.0, "operations": 30.0, "maintenance": 10.0, "interest": 5.0},
        "netHistory": [], "population": 100000.0, "approval": 55.0, "transitShare": 0.3,
        "coverage": 0.4, "dailyTransitTrips": 5000.0, "unlockedModes": ["bus"],
        "stations": [], "tracks": [], "routes": [], "activeEvents": [],
        "fieldsVersion": 1, "bankrupt": false, "commandCount": 0
    }"##
}

#[test]
fn legacy_payload_without_weather_still_parses() {
    let state: UiState =
        serde_json::from_str(legacy_ui_state_json()).expect("legacy payload must deserialize");
    assert_eq!(state.weather_state, None);
    assert_eq!(state.weather_intensity, None);
    assert_eq!(state.weather_season, None);
    assert_eq!(state.weather_event, None);
}

#[test]
fn new_payload_with_weather_parses() {
    let json = r##"{
        "tick": 600, "insights": [], "day": 3, "speed": 10.0, "cash": 12345.0,
        "loanBalance": 0.0,
        "lastDay": {"fares": 100.0, "subsidy": 50.0, "operations": 30.0, "maintenance": 10.0, "interest": 5.0},
        "netHistory": [], "population": 100000.0, "approval": 55.0, "transitShare": 0.3,
        "coverage": 0.4, "dailyTransitTrips": 5000.0, "unlockedModes": ["bus"],
        "stations": [], "tracks": [], "routes": [], "activeEvents": [],
        "fieldsVersion": 1, "bankrupt": false, "commandCount": 0,
        "weatherState": "snow", "weatherIntensity": 0.8, "weatherSeason": "winter",
        "weatherEvent": "blizzard"
    }"##;
    let state: UiState = serde_json::from_str(json).expect("new payload must deserialize");
    assert_eq!(state.weather_state, Some(WeatherState::Snow));
    assert_eq!(state.weather_intensity, Some(0.8));
    assert_eq!(state.weather_season, Some(Season::Winter));
    assert_eq!(state.weather_event, Some(WeatherEvent::Blizzard));
}

#[test]
fn weather_without_event_defaults_event_to_none() {
    let json = r##"{
        "tick": 600, "insights": [], "day": 3, "speed": 10.0, "cash": 0.0,
        "loanBalance": 0.0,
        "lastDay": {"fares": 0.0, "subsidy": 0.0, "operations": 0.0, "maintenance": 0.0, "interest": 0.0},
        "netHistory": [], "population": 0.0, "approval": 50.0, "transitShare": 0.0,
        "coverage": 0.0, "dailyTransitTrips": 0.0, "unlockedModes": [],
        "stations": [], "tracks": [], "routes": [], "activeEvents": [],
        "fieldsVersion": 1, "bankrupt": false, "commandCount": 0,
        "weatherState": "rain", "weatherIntensity": 0.5, "weatherSeason": "spring"
    }"##;
    let state: UiState = serde_json::from_str(json).expect("must deserialize");
    assert_eq!(state.weather_state, Some(WeatherState::Rain));
    assert_eq!(state.weather_event, None);
}

#[test]
fn weather_fields_round_trip_through_serialize() {
    let json = r##"{
        "tick": 1, "insights": [], "day": 1, "speed": 1.0, "cash": 0.0, "loanBalance": 0.0,
        "lastDay": {"fares": 0.0, "subsidy": 0.0, "operations": 0.0, "maintenance": 0.0, "interest": 0.0},
        "netHistory": [], "population": 0.0, "approval": 50.0, "transitShare": 0.0, "coverage": 0.0,
        "dailyTransitTrips": 0.0, "unlockedModes": [], "stations": [], "tracks": [], "routes": [],
        "activeEvents": [], "fieldsVersion": 1, "bankrupt": false, "commandCount": 0,
        "weatherState": "storm", "weatherIntensity": 0.95, "weatherSeason": "summer",
        "weatherEvent": "heatwave"
    }"##;
    let state: UiState = serde_json::from_str(json).expect("decode");
    let reencoded = serde_json::to_string(&state).expect("encode");
    let state2: UiState = serde_json::from_str(&reencoded).expect("re-decode");
    assert_eq!(state, state2);
    assert_eq!(state2.weather_state, Some(WeatherState::Storm));
    assert_eq!(state2.weather_event, Some(WeatherEvent::Heatwave));
    assert_eq!(state2.weather_season, Some(Season::Summer));
}
