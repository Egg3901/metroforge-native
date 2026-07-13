//! Fixture-based round-trip tests: binary msgType decode -> encode -> byte
//! equality, and JSON literals (copied from the shapes described by
//! `protocol.ts`/`types.ts`) decode -> encode -> value equality.

use mf_protocol::binary::{
    decode_binary, BinaryError, BinaryMsg, Fields, FrameSnapshot, MaskWhich, StaticBuildings,
    StaticElevation, StaticMask, Traffic,
};
use mf_protocol::envelope::{Envelope, FromSimJson, ToSim};
use mf_protocol::types::{Command, Difficulty, TransitMode, UiState};
use mf_protocol::Vec2;

// ---------------------------------------------------------------------------
// Binary fixtures
// ---------------------------------------------------------------------------

fn push_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn push_u16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn push_f32(v: &mut Vec<u8>, x: f32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn push_i16(v: &mut Vec<u8>, x: i16) {
    v.extend_from_slice(&x.to_le_bytes());
}
/// Push one wire-version-1 `StaticBuildings` per-building record:
/// vertexCount, flags=0, heightDm, then `(xHalfM, yHalfM)` per vertex.
fn push_building(v: &mut Vec<u8>, height_dm: u16, verts_half: &[(i16, i16)]) {
    v.push(verts_half.len() as u8);
    v.push(0); // flags
    push_u16(v, height_dm);
    for &(x, y) in verts_half {
        push_i16(v, x);
        push_i16(v, y);
    }
}

/// Push one wire-version-2 `StaticBuildings` per-building record:
/// vertexCount, flags=0, heightDm, minHeightDm, then `(xHalfM, yHalfM)` per
/// vertex — the version-2 stride adds `minHeightDm` after `heightDm`
/// (building:part stacking; see `BuildingFootprint::min_height_dm`).
fn push_building_v2(
    v: &mut Vec<u8>,
    height_dm: u16,
    min_height_dm: u16,
    verts_half: &[(i16, i16)],
) {
    v.push(verts_half.len() as u8);
    v.push(0); // flags
    push_u16(v, height_dm);
    push_u16(v, min_height_dm);
    for &(x, y) in verts_half {
        push_i16(v, x);
        push_i16(v, y);
    }
}

#[test]
fn frame_snapshot_roundtrip() {
    // header (24B) + colorTable[2] + vehicles[1*6] + agents[2*3]
    let mut b = vec![1u8 /* msgType */, 1u8 /* version */];
    push_u16(&mut b, 0); // reserved
    push_u32(&mut b, 4242); // tick
    push_u32(&mut b, 1); // vehicleCount
    push_u32(&mut b, 2); // agentCount
    push_u32(&mut b, 2); // colorTableLen
    push_u32(&mut b, 0); // reserved
    push_u32(&mut b, 0x00FF3B30); // colorTable[0]
    push_u32(&mut b, 0x00007AFF); // colorTable[1]
                                  // vehicles: [id,x,y,heading,occupancy,routeColorIdx]
    for v in [7.0f32, 100.5, -20.25, 1.57, 0.8, 1.0] {
        push_f32(&mut b, v);
    }
    // agents: [x,y,phase] * 2
    for v in [10.0f32, 20.0, 0.0, 30.0, 40.0, 2.0] {
        push_f32(&mut b, v);
    }

    let decoded = FrameSnapshot::decode(&b).expect("decode");
    assert_eq!(decoded.tick, 4242);
    assert_eq!(decoded.vehicle_count, 1);
    assert_eq!(decoded.agent_count, 2);
    assert_eq!(decoded.color_table, vec![0x00FF3B30, 0x00007AFF]);
    assert_eq!(decoded.vehicles.len(), 6);
    assert_eq!(decoded.agents.len(), 6);

    let re_encoded = decoded.encode();
    assert_eq!(re_encoded, b);

    match decode_binary(&b).unwrap() {
        BinaryMsg::Frame(f) => assert_eq!(f, decoded),
        other => panic!("expected Frame, got {other:?}"),
    }
}

#[test]
fn fields_roundtrip() {
    let n: usize = 3;
    let mut b = vec![2u8, 1u8];
    push_u16(&mut b, 0);
    push_u32(&mut b, 9); // fieldsVersion
    push_u32(&mut b, n as u32); // cellCount
    push_u32(&mut b, 0);
    for v in [0.1f32, 0.2, 0.3] {
        push_f32(&mut b, v); // terrain
    }
    for v in [1.0f32, 2.0, 3.0] {
        push_f32(&mut b, v); // population
    }
    for v in [4.0f32, 5.0, 6.0] {
        push_f32(&mut b, v); // jobs
    }
    for v in [0.5f32, 1.5, 2.5] {
        push_f32(&mut b, v); // landValue
    }
    b.extend_from_slice(&[0u8, 1, 0]); // water
    b.extend_from_slice(&[1u8, 0, 1]); // parks

    let decoded = Fields::decode(&b).expect("decode");
    assert_eq!(decoded.version, 9);
    assert_eq!(decoded.cell_count, 3);
    assert_eq!(decoded.terrain, vec![0.1, 0.2, 0.3]);
    assert_eq!(decoded.water, vec![0, 1, 0]);
    assert_eq!(decoded.parks, vec![1, 0, 1]);

    assert_eq!(decoded.encode(), b);
    match decode_binary(&b).unwrap() {
        BinaryMsg::Fields(f) => assert_eq!(f, decoded),
        other => panic!("expected Fields, got {other:?}"),
    }
}

#[test]
fn traffic_roundtrip() {
    let w = 2u32;
    let h = 2u32;
    let mut b = vec![3u8, 1u8];
    push_u16(&mut b, 1); // hotspotCount
    push_u32(&mut b, w);
    push_u32(&mut b, h);
    push_f32(&mut b, 50.0); // cellSize
    push_f32(&mut b, -100.0); // originX
    push_f32(&mut b, -100.0); // originY
    push_u32(&mut b, w * h); // valueCount
    push_u32(&mut b, 0);
    for v in [0.1f32, 0.2, 0.3, 0.4] {
        push_f32(&mut b, v);
    }
    // one hotspot (x,y,severity)
    push_f32(&mut b, 12.0);
    push_f32(&mut b, 34.0);
    push_f32(&mut b, 0.9);

    let decoded = Traffic::decode(&b).expect("decode");
    assert_eq!(decoded.w, 2);
    assert_eq!(decoded.h, 2);
    assert_eq!(decoded.values, vec![0.1, 0.2, 0.3, 0.4]);
    assert_eq!(decoded.hotspots.len(), 1);
    assert_eq!(decoded.hotspots[0].severity, 0.9);

    assert_eq!(decoded.encode(), b);
    match decode_binary(&b).unwrap() {
        BinaryMsg::Traffic(t) => assert_eq!(t, decoded),
        other => panic!("expected Traffic, got {other:?}"),
    }
}

#[test]
fn static_mask_roundtrip() {
    let res = 4u32;
    let mut b = vec![4u8, 1u8, 1u8 /* which=park */, 0u8 /* reserved */];
    push_u32(&mut b, res);
    push_u32(&mut b, 0);
    b.extend_from_slice(&[1u8; 16]); // res*res mask bytes

    let decoded = StaticMask::decode(&b).expect("decode");
    assert_eq!(decoded.which, MaskWhich::Park);
    assert_eq!(decoded.res, 4);
    assert_eq!(decoded.mask.len(), 16);

    assert_eq!(decoded.encode(), b);
    match decode_binary(&b).unwrap() {
        BinaryMsg::Mask(m) => assert_eq!(m, decoded),
        other => panic!("expected Mask, got {other:?}"),
    }
}

#[test]
fn static_elevation_roundtrip() {
    let res = 3u32;
    // signed meters incl. negatives (below sea level) and a big peak.
    let heights: Vec<i16> = vec![-5, 0, 12, 340, -32768, 32767, 100, 200, 300];
    let src = StaticElevation {
        res,
        heights: heights.clone(),
    };
    let bytes = src.encode();
    // header: msgType=7, version=1, reserved u16, res u32, reserved u32
    assert_eq!(bytes[0], 7);
    assert_eq!(bytes[1], 1);
    assert_eq!(bytes.len(), 12 + (res * res) as usize * 2);

    let decoded = StaticElevation::decode(&bytes).expect("decode");
    assert_eq!(decoded.res, res);
    assert_eq!(decoded.heights, heights);
    assert_eq!(decoded.encode(), bytes);

    match decode_binary(&bytes).unwrap() {
        BinaryMsg::Elevation(e) => assert_eq!(e, decoded),
        other => panic!("expected Elevation, got {other:?}"),
    }
}

#[test]
fn static_elevation_truncated_body_errors() {
    // res=2 declares 4 samples (8 body bytes) but only 2 provided.
    let mut b = vec![7u8, 1u8];
    push_u16(&mut b, 0);
    push_u32(&mut b, 2);
    push_u32(&mut b, 0);
    push_i16(&mut b, 42);
    assert!(matches!(
        StaticElevation::decode(&b),
        Err(BinaryError::TooShort { .. })
    ));
}

#[test]
fn static_buildings_v1_empty_decodes_and_reencodes_as_v2() {
    // header only: 0 buildings, vertexTotal 0, wire version 1.
    let mut b = vec![5u8, 1u8];
    push_u16(&mut b, 0); // reserved
    push_u32(&mut b, 0); // buildingCount
    push_u32(&mut b, 0); // vertexTotal

    let decoded = StaticBuildings::decode(&b).expect("decode");
    assert_eq!(decoded.buildings.len(), 0);

    // encode() always emits wire version 2 (see its doc comment); for an
    // empty building list the only difference from the v1 input is that
    // version byte, since there are no buildings for the wider per-building
    // stride to apply to.
    let mut expected_reencode = b.clone();
    expected_reencode[1] = 2;
    assert_eq!(decoded.encode(), expected_reencode);
    match decode_binary(&b).unwrap() {
        BinaryMsg::Buildings(sb) => assert_eq!(sb, decoded),
        other => panic!("expected Buildings, got {other:?}"),
    }
}

#[test]
fn static_buildings_v1_two_buildings_decode_with_zero_min_height() {
    // Wire version 1 has no `minHeightDm` field at all: decode must fill it
    // with 0 ("starts at ground") for every building rather than error.
    // Building 0: a triangle, heightDm=250 (25.0m), includes negative coords.
    // Building 1: a quad, heightDm=0 ("unknown", renderer falls back).
    let b0_verts = [(-20i16, -20i16), (20, -20), (0, 40)];
    let b1_verts = [(-10i16, -10i16), (10, -10), (10, 10), (-10, 10)];
    let vertex_total = (b0_verts.len() + b1_verts.len()) as u32;

    let mut b = vec![5u8, 1u8];
    push_u16(&mut b, 0); // reserved
    push_u32(&mut b, 2); // buildingCount
    push_u32(&mut b, vertex_total);
    push_building(&mut b, 250, &b0_verts);
    push_building(&mut b, 0, &b1_verts);

    let decoded = StaticBuildings::decode(&b).expect("decode");
    assert_eq!(decoded.buildings.len(), 2);
    assert_eq!(decoded.buildings[0].height_dm, 250);
    assert_eq!(decoded.buildings[0].min_height_dm, 0);
    assert_eq!(
        decoded.buildings[0].verts,
        vec![[-10.0, -10.0], [10.0, -10.0], [0.0, 20.0]]
    );
    assert_eq!(decoded.buildings[1].height_dm, 0);
    assert_eq!(decoded.buildings[1].min_height_dm, 0);
    assert_eq!(
        decoded.buildings[1].verts,
        vec![[-5.0, -5.0], [5.0, -5.0], [5.0, 5.0], [-5.0, 5.0]]
    );

    match decode_binary(&b).unwrap() {
        BinaryMsg::Buildings(sb) => assert_eq!(sb, decoded),
        other => panic!("expected Buildings, got {other:?}"),
    }
}

#[test]
fn static_buildings_v2_roundtrip_byte_exact_with_nonzero_min_height() {
    // building:part stacking fixture: a podium (min=0, i.e. ground-based)
    // and a tower set back on top of it (min=80dm = 8.0m > 0).
    let podium_verts = [(-20i16, -20i16), (20, -20), (20, 20), (-20, 20)];
    let tower_verts = [(-10i16, -10i16), (10, -10), (10, 10), (-10, 10)];
    let vertex_total = (podium_verts.len() + tower_verts.len()) as u32;

    let mut b = vec![5u8, 2u8];
    push_u16(&mut b, 0); // reserved
    push_u32(&mut b, 2); // buildingCount
    push_u32(&mut b, vertex_total);
    push_building_v2(&mut b, 80, 0, &podium_verts);
    push_building_v2(&mut b, 300, 80, &tower_verts);

    let decoded = StaticBuildings::decode(&b).expect("decode");
    assert_eq!(decoded.buildings.len(), 2);
    assert_eq!(decoded.buildings[0].height_dm, 80);
    assert_eq!(decoded.buildings[0].min_height_dm, 0);
    assert_eq!(decoded.buildings[1].height_dm, 300);
    assert_eq!(decoded.buildings[1].min_height_dm, 80);

    // Byte-exact: every fixture value here started life as a half-meter or
    // decimeter integer, and encode() always emits version 2.
    assert_eq!(decoded.encode(), b);
    match decode_binary(&b).unwrap() {
        BinaryMsg::Buildings(sb) => assert_eq!(sb, decoded),
        other => panic!("expected Buildings, got {other:?}"),
    }
}

#[test]
fn static_buildings_unsupported_version_errors() {
    // Version 3: neither the legacy v1 layout nor the current v2 layout.
    let mut b = vec![5u8, 3u8];
    push_u16(&mut b, 0);
    push_u32(&mut b, 0);
    push_u32(&mut b, 0);
    match StaticBuildings::decode(&b) {
        Err(BinaryError::UnsupportedVersion(3)) => {}
        other => panic!("expected UnsupportedVersion(3), got {other:?}"),
    }
}

#[test]
fn frame_snapshot_still_rejects_non_v1_version() {
    // Regression check for the check_msg_type refactor: every msgType other
    // than 5 (StaticBuildings) must still require exactly wire version 1.
    let mut b = vec![1u8, 2u8]; // msgType=1 (Frame), version=2
    push_u16(&mut b, 0); // reserved
    push_u32(&mut b, 0); // tick
    push_u32(&mut b, 0); // vehicleCount
    push_u32(&mut b, 0); // agentCount
    push_u32(&mut b, 0); // colorTableLen
    push_u32(&mut b, 0); // reserved
    match FrameSnapshot::decode(&b) {
        Err(BinaryError::UnsupportedVersion(2)) => {}
        other => panic!("expected UnsupportedVersion(2), got {other:?}"),
    }
}

#[test]
fn static_buildings_truncated_buffer_errors() {
    let b0_verts = [(-20i16, -20i16), (20, -20), (0, 40)];
    let mut b = vec![5u8, 1u8];
    push_u16(&mut b, 0);
    push_u32(&mut b, 1); // buildingCount = 1
    push_u32(&mut b, 3); // vertexTotal = 3
    push_building(&mut b, 100, &b0_verts);

    // Chop off the last vertex's y half.
    b.truncate(b.len() - 2);

    match StaticBuildings::decode(&b) {
        Err(BinaryError::TooShort { .. }) => {}
        other => panic!("expected TooShort, got {other:?}"),
    }
}

#[test]
fn static_buildings_vertex_count_out_of_range_errors() {
    // vertexCount = 2 (below the 3..=64 minimum).
    let mut too_few = vec![5u8, 1u8];
    push_u16(&mut too_few, 0);
    push_u32(&mut too_few, 1);
    push_u32(&mut too_few, 2);
    push_building(&mut too_few, 100, &[(0, 0), (1, 1)]);
    match StaticBuildings::decode(&too_few) {
        Err(BinaryError::InvalidVertexCount { got: 2 }) => {}
        other => panic!("expected InvalidVertexCount{{got:2}}, got {other:?}"),
    }

    // vertexCount = 65 (above the 3..=64 maximum). Only the 4-byte building
    // header needs to exist on the wire for decode to reject it before
    // trying to read 65 vertices.
    let mut too_many = vec![5u8, 1u8];
    push_u16(&mut too_many, 0);
    push_u32(&mut too_many, 1);
    push_u32(&mut too_many, 65);
    too_many.push(65); // vertexCount
    too_many.push(0); // flags
    push_u16(&mut too_many, 100); // heightDm
    match StaticBuildings::decode(&too_many) {
        Err(BinaryError::InvalidVertexCount { got: 65 }) => {}
        other => panic!("expected InvalidVertexCount{{got:65}}, got {other:?}"),
    }
}

#[test]
fn static_buildings_vertex_total_mismatch_errors() {
    let b0_verts = [(-20i16, -20i16), (20, -20), (0, 40)];
    let mut b = vec![5u8, 1u8];
    push_u16(&mut b, 0);
    push_u32(&mut b, 1); // buildingCount = 1
    push_u32(&mut b, 4); // vertexTotal = 4, but the one building has 3
    push_building(&mut b, 100, &b0_verts);

    match StaticBuildings::decode(&b) {
        Err(BinaryError::VertexTotalMismatch {
            declared: 4,
            actual: 3,
        }) => {}
        other => panic!("expected VertexTotalMismatch{{4,3}}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// JSON fixtures
// ---------------------------------------------------------------------------

#[test]
fn command_build_station_roundtrip() {
    let json = r#"{"kind":"buildStation","mode":"bus","pos":{"x":120.5,"y":-40.25}}"#;
    let cmd: Command = serde_json::from_str(json).expect("decode");
    match &cmd {
        Command::BuildStation { mode, pos } => {
            assert_eq!(*mode, TransitMode::Bus);
            assert_eq!(
                *pos,
                Vec2 {
                    x: 120.5,
                    y: -40.25
                }
            );
        }
        other => panic!("expected BuildStation, got {other:?}"),
    }
    let re: serde_json::Value = serde_json::to_value(&cmd).unwrap();
    let expected: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(re, expected);
}

#[test]
fn command_build_track_roundtrip() {
    let json = r#"{"kind":"buildTrack","mode":"metro","grade":"tunnel","fromStationId":3,"toStationId":9,"waypoints":[{"x":1.0,"y":2.0},{"x":3.0,"y":4.0}]}"#;
    let cmd: Command = serde_json::from_str(json).expect("decode");
    match &cmd {
        Command::BuildTrack {
            mode,
            grade,
            from_station_id,
            to_station_id,
            waypoints,
        } => {
            assert_eq!(*mode, TransitMode::Metro);
            assert_eq!(*from_station_id, 3);
            assert_eq!(*to_station_id, 9);
            assert_eq!(waypoints.len(), 2);
            let _ = grade;
        }
        other => panic!("expected BuildTrack, got {other:?}"),
    }
    let re: serde_json::Value = serde_json::to_value(&cmd).unwrap();
    let expected: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(re, expected);
}

#[test]
fn command_edit_route_omits_absent_optionals() {
    let json = r#"{"kind":"editRoute","routeId":5,"fare":2.5}"#;
    let cmd: Command = serde_json::from_str(json).expect("decode");
    let re = serde_json::to_string(&cmd).unwrap();
    let re_value: serde_json::Value = serde_json::from_str(&re).unwrap();
    let expected: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(re_value, expected, "re-encoded: {re}");
}

#[test]
fn ui_state_roundtrip_from_realistic_literal() {
    // Shaped like a real sidecar `ui` payload (2 Hz `UiState`), field-for-field
    // per metroforge/src/host/protocol.ts:46-78.
    let json = r##"{
        "tick": 12345,
        "insights": ["Approval is falling in the east side."],
        "day": 10,
        "speed": 30,
        "cash": 154302.5,
        "loanBalance": 0,
        "lastDay": {"fares": 1200.0, "subsidy": 300.0, "operations": -800.0, "maintenance": -150.0, "interest": 0.0},
        "netHistory": [100.0, 150.0, 200.0],
        "population": 84000,
        "approval": 62.5,
        "transitShare": 0.18,
        "coverage": 0.42,
        "dailyTransitTrips": 15234,
        "unlockedModes": ["bus", "tram"],
        "stations": [
            {"id": 1, "name": "Union Sq", "x": 10.0, "y": 20.0, "mode": "bus", "level": 2, "ridership": 500.0, "alightings": 480.0}
        ],
        "tracks": [
            {"id": 1, "mode": "bus", "grade": "surface", "points": [10.0, 20.0, 30.0, 40.0], "fromStationId": 1, "toStationId": 2}
        ],
        "routes": [
            {"id": 1, "name": "Blue Line", "color": "#007aff", "mode": "bus", "stationIds": [1,2], "headwaySeconds": 300, "fare": 2.5, "vehicleCount": 4, "dailyRidership": 900.0, "dailyRevenue": 2250.0, "lengthMeters": 5400.0, "capacity": 3000.0, "load": 900.0, "crowding": 0.3, "segmentLoads": [900.0]}
        ],
        "activeEvents": [{"id": "festival", "name": "Harbor Festival", "daysLeft": 2}],
        "fieldsVersion": 4,
        "bankrupt": false,
        "failed": null,
        "maxDay": null,
        "eraLabel": null,
        "commandCount": 7
    }"##;

    let ui: UiState = serde_json::from_str(json).expect("decode UiState");
    assert_eq!(ui.tick, 12345);
    assert_eq!(ui.stations.len(), 1);
    assert_eq!(ui.stations[0].name, "Union Sq");
    assert_eq!(ui.routes[0].color, "#007aff");
    assert!(ui.failed.is_none());

    // UiState is receive-only (the client never re-encodes it to send), so we
    // don't require byte-identical JSON on re-serialize: whole-number fields
    // typed `f64` legitimately round-trip as e.g. `15234.0` instead of the
    // sidecar's `15234`. What matters is that re-decoding the re-encoded form
    // reproduces the same struct.
    let re_json = serde_json::to_string(&ui).unwrap();
    let re_decoded: UiState = serde_json::from_str(&re_json).expect("re-decode");
    assert_eq!(re_decoded, ui);
}

#[test]
fn envelope_hello_and_init_roundtrip() {
    let hello = ToSim::Hello(mf_protocol::ClientHelloPayload {
        client_protocol_version: 1,
    });
    let env = hello.to_envelope();
    assert_eq!(env.t, "hello");
    assert_eq!(env.seq, None);
    let json = serde_json::to_string(&env).unwrap();
    assert!(json.contains("\"clientProtocolVersion\":1"));

    let init_json = r#"{"t":"init","p":{"seed":12345,"difficulty":"normal","presetKey":"nyc"}}"#;
    let env: Envelope = serde_json::from_str(init_json).unwrap();
    assert_eq!(env.t, "init");
    let init: mf_protocol::InitPayload = serde_json::from_value(env.p.unwrap()).unwrap();
    assert_eq!(init.seed, 12345);
    assert_eq!(init.difficulty, Difficulty::Normal);
    assert_eq!(init.preset_key.as_deref(), Some("nyc"));
}

#[test]
fn envelope_hello_accepts_enriched_city_list_fields() {
    let hello_json = r#"{"t":"hello","p":{"protocolVersion":1,"gameVersion":"0.5.0","cityList":[{"key":"cleveland","label":"Cleveland","country":"USA","population":372000,"buildingCount":33553,"sizeKm":12.0}],"defaultWorldSize":12000.0}}"#;
    let env: Envelope = serde_json::from_str(hello_json).unwrap();
    let msg = FromSimJson::from_envelope(env).unwrap();
    match msg {
        FromSimJson::Hello(h) => {
            let c = &h.city_list[0];
            assert_eq!(c.key, "cleveland");
            assert_eq!(c.country.as_deref(), Some("USA"));
            assert_eq!(c.population, Some(372_000.0));
            assert_eq!(c.building_count, Some(33_553));
            assert_eq!(c.size_km, Some(12.0));
        }
        other => panic!("expected Hello, got {other:?}"),
    }
}

#[test]
fn envelope_ready_hello_and_toast_roundtrip() {
    let hello_json = r#"{"t":"hello","p":{"protocolVersion":1,"gameVersion":"0.1.0","cityList":[{"key":"nyc","label":"New York City"}],"defaultWorldSize":24000.0}}"#;
    let env: Envelope = serde_json::from_str(hello_json).unwrap();
    let msg = FromSimJson::from_envelope(env).unwrap();
    match msg {
        FromSimJson::Hello(h) => {
            assert_eq!(h.protocol_version, 1);
            assert_eq!(h.city_list[0].key, "nyc");
            assert!(h.city_list[0].population.is_none());
        }
        other => panic!("expected Hello, got {other:?}"),
    }

    let toast_json = r#"{"t":"toast","p":{"message":"Bankrupt!","tone":"warn"}}"#;
    let env: Envelope = serde_json::from_str(toast_json).unwrap();
    match FromSimJson::from_envelope(env).unwrap() {
        FromSimJson::Toast(t) => assert_eq!(t.message, "Bankrupt!"),
        other => panic!("expected Toast, got {other:?}"),
    }

    let command_result_json =
        r#"{"t":"commandResult","seq":42,"p":{"result":{"ok":true,"createdId":7}}}"#;
    let env: Envelope = serde_json::from_str(command_result_json).unwrap();
    match FromSimJson::from_envelope(env).unwrap() {
        FromSimJson::CommandResult { seq, result } => {
            assert_eq!(seq, Some(42));
            assert!(result.ok);
            assert_eq!(result.created_id, Some(7));
        }
        other => panic!("expected CommandResult, got {other:?}"),
    }

    let bye_json = r#"{"t":"bye"}"#;
    let env: Envelope = serde_json::from_str(bye_json).unwrap();
    assert_eq!(FromSimJson::from_envelope(env).unwrap(), FromSimJson::Bye);
}

// ---------------------------------------------------------------------------
// Exhaustive FromSimJson coverage + degrade-on-malformed
// ---------------------------------------------------------------------------

fn parse_from_sim(json: &str) -> Result<FromSimJson, mf_protocol::envelope::EnvelopeError> {
    let env: Envelope = serde_json::from_str(json).expect("envelope JSON itself must parse");
    FromSimJson::from_envelope(env)
}

fn minimal_static_city_json() -> &'static str {
    r#"{"fieldW":2,"fieldH":2,"cellSize":10.0,"originX":0.0,"originY":0.0,"worldSize":20.0,"roadScale":1.0,"roads":[]}"#
}

fn minimal_ui_state_json() -> &'static str {
    r#"{
        "tick": 1,
        "insights": [],
        "day": 0,
        "speed": 1,
        "cash": 0,
        "loanBalance": 0,
        "lastDay": {"fares":0,"subsidy":0,"operations":0,"maintenance":0,"interest":0},
        "netHistory": [],
        "population": 0,
        "approval": 50,
        "transitShare": 0,
        "coverage": 0,
        "dailyTransitTrips": 0,
        "unlockedModes": [],
        "stations": [],
        "tracks": [],
        "routes": [],
        "activeEvents": [],
        "fieldsVersion": 0,
        "bankrupt": false,
        "commandCount": 0
    }"#
}

#[test]
fn from_sim_json_every_variant_roundtrips() {
    // Hello
    let hello = r#"{"t":"hello","p":{"protocolVersion":1,"gameVersion":"0.5.0","cityList":[{"key":"nyc","label":"NYC"}],"defaultWorldSize":24000.0}}"#;
    match parse_from_sim(hello).unwrap() {
        FromSimJson::Hello(h) => {
            assert_eq!(h.protocol_version, 1);
            assert_eq!(h.city_list[0].key, "nyc");
        }
        other => panic!("Hello: {other:?}"),
    }

    // Ready
    let ready = format!(
        r#"{{"t":"ready","p":{{"staticCity":{}}}}}"#,
        minimal_static_city_json()
    );
    match parse_from_sim(&ready).unwrap() {
        FromSimJson::Ready(r) => assert_eq!(r.static_city.field_w, 2),
        other => panic!("Ready: {other:?}"),
    }

    // Demand
    let demand = r#"{"t":"demand","p":{"lines":[{"x1":0,"y1":0,"x2":1,"y2":1,"weight":2.5,"share":0.1}],"maxWeight":2.5}}"#;
    match parse_from_sim(demand).unwrap() {
        FromSimJson::Demand(d) => {
            assert_eq!(d.lines.len(), 1);
            assert!((d.max_weight - 2.5).abs() < 1e-9);
        }
        other => panic!("Demand: {other:?}"),
    }

    // Ui
    let ui = format!(r#"{{"t":"ui","p":{}}}"#, minimal_ui_state_json());
    match parse_from_sim(&ui).unwrap() {
        FromSimJson::Ui(u) => {
            assert_eq!(u.tick, 1);
            // Optional sim-depth fields omitted → defaults, not panic.
            assert!(u.hour_of_day.is_none());
            assert!(u.districts.is_empty());
        }
        other => panic!("Ui: {other:?}"),
    }

    // CommandResult (already covered above; keep in the exhaustive set)
    let cr = r#"{"t":"commandResult","seq":9,"p":{"result":{"ok":false,"error":"nope"}}}"#;
    match parse_from_sim(cr).unwrap() {
        FromSimJson::CommandResult { seq, result } => {
            assert_eq!(seq, Some(9));
            assert!(!result.ok);
            assert_eq!(result.error.as_deref(), Some("nope"));
        }
        other => panic!("CommandResult: {other:?}"),
    }

    // TrackCost
    let tc = r#"{"t":"trackCost","seq":3,"p":{"cost":1234.5}}"#;
    match parse_from_sim(tc).unwrap() {
        FromSimJson::TrackCost {
            seq,
            cost,
            breakdown,
        } => {
            assert_eq!(seq, Some(3));
            assert!((cost - 1234.5).abs() < 1e-9);
            assert!(breakdown.is_none());
        }
        other => panic!("TrackCost: {other:?}"),
    }

    // Saved
    let saved = r#"{"t":"saved","p":{"json":"{\"tick\":1}"}}"#;
    match parse_from_sim(saved).unwrap() {
        FromSimJson::Saved(s) => assert!(s.json.contains("tick")),
        other => panic!("Saved: {other:?}"),
    }

    // Replay
    let replay = r#"{"t":"replay","p":{"seed":1,"difficulty":"normal","commandLog":[],"finalTick":10,"stateHash":0,"scoreHint":0.0}}"#;
    match parse_from_sim(replay).unwrap() {
        FromSimJson::Replay(r) => {
            assert_eq!(r.seed, 1);
            assert_eq!(r.final_tick, 10);
        }
        other => panic!("Replay: {other:?}"),
    }

    // Toast / Pong / Bye
    match parse_from_sim(r#"{"t":"toast","p":{"message":"hi","tone":"info"}}"#).unwrap() {
        FromSimJson::Toast(t) => assert_eq!(t.message, "hi"),
        other => panic!("Toast: {other:?}"),
    }
    assert_eq!(
        parse_from_sim(r#"{"t":"pong"}"#).unwrap(),
        FromSimJson::Pong
    );
    assert_eq!(parse_from_sim(r#"{"t":"bye"}"#).unwrap(), FromSimJson::Bye);
}

#[test]
fn from_sim_json_missing_payload_degrades_to_error() {
    for t in [
        "hello",
        "ready",
        "demand",
        "ui",
        "commandResult",
        "trackCost",
        "saved",
        "replay",
        "toast",
    ] {
        let json = format!(r#"{{"t":"{t}"}}"#);
        let err = parse_from_sim(&json).expect_err("missing p must not panic");
        match err {
            mf_protocol::envelope::EnvelopeError::MissingPayload(name) => {
                assert_eq!(name, t);
            }
            other => panic!("expected MissingPayload for {t}, got {other:?}"),
        }
    }
    // Payloadless messages still succeed without `p`.
    assert_eq!(
        parse_from_sim(r#"{"t":"pong"}"#).unwrap(),
        FromSimJson::Pong
    );
    assert_eq!(parse_from_sim(r#"{"t":"bye"}"#).unwrap(), FromSimJson::Bye);
}

#[test]
fn from_sim_json_malformed_payload_degrades_to_error() {
    // Wrong shape / missing required fields → BadPayload, never panic.
    let cases = [
        r#"{"t":"hello","p":{"protocolVersion":1}}"#, // missing cityList etc.
        r#"{"t":"toast","p":{"message":"x"}}"#,       // missing tone
        r#"{"t":"trackCost","p":{}}"#,                // missing cost
        r#"{"t":"ui","p":{"tick":1}}"#,               // missing most UiState fields
        r#"{"t":"demand","p":{"lines":"nope","maxWeight":1}}"#,
    ];
    for json in cases {
        let err = parse_from_sim(json).expect_err("bad payload must not panic");
        assert!(
            matches!(err, mf_protocol::envelope::EnvelopeError::BadPayload(_, _)),
            "expected BadPayload for {json}, got {err:?}"
        );
    }
}

#[test]
fn from_sim_json_unknown_type_degrades_to_error() {
    let err = parse_from_sim(r#"{"t":"notARealMessage","p":{}}"#).unwrap_err();
    match err {
        mf_protocol::envelope::EnvelopeError::UnknownType(t) => {
            assert_eq!(t, "notARealMessage");
        }
        other => panic!("expected UnknownType, got {other:?}"),
    }
}

#[test]
fn ui_state_omitted_optional_fields_default_not_panic() {
    // Sim-depth optionals + failed/maxDay/eraLabel all default when absent.
    let json = minimal_ui_state_json();
    let ui: UiState = serde_json::from_str(json).expect("must accept legacy shape");
    assert!(ui.failed.is_none());
    assert!(ui.max_day.is_none());
    assert!(ui.era_label.is_none());
    assert!(ui.hour_of_day.is_none());
    assert!(ui.demand_factor.is_none());
    assert!(ui.districts.is_empty());
    assert!(ui.overcrowded_routes.is_none());
}

// ---------------------------------------------------------------------------
// Binary decoder fuzz-ish edge cases (truncated / zero / max counts)
// ---------------------------------------------------------------------------

#[test]
fn decode_binary_empty_and_unknown_msg_type() {
    assert!(matches!(
        decode_binary(&[]),
        Err(BinaryError::TooShort { .. })
    ));
    assert!(matches!(
        decode_binary(&[99]),
        Err(BinaryError::UnknownMsgType(99))
    ));
}

#[test]
fn frame_snapshot_zero_counts_and_truncated() {
    // Minimal valid frame: header only, all counts zero.
    let mut b = vec![1u8, 1u8];
    push_u16(&mut b, 0);
    push_u32(&mut b, 7); // tick
    push_u32(&mut b, 0); // vehicleCount
    push_u32(&mut b, 0); // agentCount
    push_u32(&mut b, 0); // colorTableLen
    push_u32(&mut b, 0); // reserved
    let decoded = FrameSnapshot::decode(&b).expect("zero-count frame");
    assert_eq!(decoded.tick, 7);
    assert!(decoded.vehicles.is_empty());
    assert!(decoded.agents.is_empty());
    assert!(decoded.color_table.is_empty());
    assert_eq!(decoded.encode(), b);

    // Truncated mid-header.
    assert!(matches!(
        FrameSnapshot::decode(&b[..10]),
        Err(BinaryError::TooShort { .. })
    ));

    // Declares one vehicle but body is missing → TooShort, not panic.
    let mut short = b.clone();
    short[8..12].copy_from_slice(&1u32.to_le_bytes()); // vehicleCount = 1
    assert!(matches!(
        FrameSnapshot::decode(&short),
        Err(BinaryError::TooShort { .. })
    ));

    // "Max" count relative to a tiny buffer: huge vehicleCount must fail
    // closed on TooShort without attempting a giant allocation past the buf.
    let mut huge = b.clone();
    huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        FrameSnapshot::decode(&huge),
        Err(BinaryError::TooShort { .. })
    ));
}

#[test]
fn fields_zero_count_and_truncated() {
    let mut b = vec![2u8, 1u8];
    push_u16(&mut b, 0);
    push_u32(&mut b, 1); // version
    push_u32(&mut b, 0); // cellCount
    push_u32(&mut b, 0);
    let decoded = Fields::decode(&b).expect("zero cells");
    assert_eq!(decoded.cell_count, 0);
    assert!(decoded.terrain.is_empty());
    assert!(decoded.water.is_empty());
    assert_eq!(decoded.encode(), b);

    assert!(matches!(
        Fields::decode(&b[..8]),
        Err(BinaryError::TooShort { .. })
    ));

    // cellCount=1 but no body.
    let mut short = b.clone();
    short[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(
        Fields::decode(&short),
        Err(BinaryError::TooShort { .. })
    ));
}

#[test]
fn traffic_zero_counts_and_truncated() {
    let mut b = vec![3u8, 1u8];
    push_u16(&mut b, 0); // hotspotCount
    push_u32(&mut b, 0); // w
    push_u32(&mut b, 0); // h
    push_f32(&mut b, 50.0);
    push_f32(&mut b, 0.0);
    push_f32(&mut b, 0.0);
    push_u32(&mut b, 0); // valueCount
    push_u32(&mut b, 0);
    let decoded = Traffic::decode(&b).expect("empty traffic");
    assert!(decoded.values.is_empty());
    assert!(decoded.hotspots.is_empty());
    assert_eq!(decoded.encode(), b);

    assert!(matches!(
        Traffic::decode(&b[..16]),
        Err(BinaryError::TooShort { .. })
    ));

    // hotspotCount=1 with no hotspot body.
    let mut short = b.clone();
    short[2..4].copy_from_slice(&1u16.to_le_bytes());
    assert!(matches!(
        Traffic::decode(&short),
        Err(BinaryError::TooShort { .. })
    ));
}

#[test]
fn static_mask_zero_res_and_truncated() {
    let mut b = vec![4u8, 1u8, 1u8, 0u8]; // which=park
    push_u32(&mut b, 0); // res=0 → 0 bytes of mask
    push_u32(&mut b, 0);
    let decoded = StaticMask::decode(&b).expect("res=0");
    assert_eq!(decoded.res, 0);
    assert!(decoded.mask.is_empty());
    assert_eq!(decoded.which, MaskWhich::Park);

    // res=2 declares 4 bytes but none follow.
    let mut short = vec![4u8, 1u8, 1u8, 0u8];
    push_u32(&mut short, 2);
    push_u32(&mut short, 0);
    assert!(matches!(
        StaticMask::decode(&short),
        Err(BinaryError::TooShort { .. })
    ));

    // Unknown which value.
    let mut bad_which = vec![4u8, 1u8, 99u8, 0u8];
    push_u32(&mut bad_which, 0);
    push_u32(&mut bad_which, 0);
    assert!(matches!(
        StaticMask::decode(&bad_which),
        Err(BinaryError::UnknownMaskWhich(99))
    ));
}

#[test]
fn static_buildings_max_vertex_count_accepted_and_huge_count_truncates() {
    // vertexCount=64 (the documented max) must decode.
    let verts: Vec<(i16, i16)> = (0..64).map(|i| (i, i)).collect();
    let mut b = vec![5u8, 1u8];
    push_u16(&mut b, 0);
    push_u32(&mut b, 1);
    push_u32(&mut b, 64);
    push_building(&mut b, 100, &verts);
    let decoded = StaticBuildings::decode(&b).expect("64-vert building");
    assert_eq!(decoded.buildings[0].verts.len(), 64);

    // Hostile buildingCount with a tiny buffer: TooShort, not panic/OOM.
    let mut hostile = vec![5u8, 1u8];
    push_u16(&mut hostile, 0);
    push_u32(&mut hostile, u32::MAX);
    push_u32(&mut hostile, 0);
    assert!(matches!(
        StaticBuildings::decode(&hostile),
        Err(BinaryError::TooShort { .. })
    ));
}
