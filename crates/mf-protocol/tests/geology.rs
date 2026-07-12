//! Geology / underground wire (v0.8). The `breakdown` on `trackCost` and the
//! whole `strataProbe` round-trip are additive: a pre-v0.8 sidecar emits a bare
//! `{cost}` and it must still decode (`breakdown == None`), while a v0.8 sidecar
//! carries the full breakdown and a `strataProbe` reply. These tests pin both
//! shapes so an old sidecar and the future render lane agree.

use mf_protocol::envelope::{Envelope, FromSimJson, StrataProbePayload, ToSim};
use mf_protocol::{StrataProbeResultPayload, TrackCostBreakdown, TrackCostPayload};
use serde_json::json;

/// A pre-v0.8 `trackCost` payload (no breakdown) still decodes.
#[test]
fn legacy_trackcost_without_breakdown_decodes() {
    let payload: TrackCostPayload =
        serde_json::from_str(r#"{"cost": 123456.0}"#).expect("legacy trackCost must decode");
    assert_eq!(payload.cost, 123456.0);
    assert!(payload.breakdown.is_none());
}

/// A v0.8 `trackCost` with a breakdown decodes with every field populated.
#[test]
fn new_trackcost_with_breakdown_decodes() {
    let json = r#"{
        "cost": 5000000.0,
        "breakdown": {
            "surface": 800000.0, "elevated": 2100000.0,
            "cutCover": 0.0, "bored": 5000000.0,
            "strata": "fill/clay/rock", "belowWaterTable": true
        }
    }"#;
    let payload: TrackCostPayload = serde_json::from_str(json).expect("must decode");
    let b = payload.breakdown.expect("breakdown present");
    assert_eq!(b.bored, 5000000.0);
    assert_eq!(b.strata, "fill/clay/rock");
    assert!(b.below_water_table);
}

/// `trackCost` decodes through the `FromSimJson` envelope, carrying the breakdown.
#[test]
fn trackcost_envelope_carries_breakdown() {
    let env = Envelope {
        t: "trackCost".to_string(),
        seq: Some(7),
        p: Some(json!({
            "cost": 42.0,
            "breakdown": {
                "surface": 10.0, "elevated": 20.0, "cutCover": 42.0, "bored": 0.0,
                "strata": "fill/clay", "belowWaterTable": false
            }
        })),
    };
    match FromSimJson::from_envelope(env).expect("decode") {
        FromSimJson::TrackCost {
            seq,
            cost,
            breakdown,
        } => {
            assert_eq!(seq, Some(7));
            assert_eq!(cost, 42.0);
            let b = breakdown.expect("breakdown");
            assert_eq!(b.cut_cover, 42.0);
            assert_eq!(b.strata, "fill/clay");
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

/// A legacy envelope with a bare cost decodes to `breakdown: None`.
#[test]
fn trackcost_envelope_legacy_none_breakdown() {
    let env = Envelope {
        t: "trackCost".to_string(),
        seq: None,
        p: Some(json!({ "cost": 99.0 })),
    };
    match FromSimJson::from_envelope(env).expect("decode") {
        FromSimJson::TrackCost {
            cost, breakdown, ..
        } => {
            assert_eq!(cost, 99.0);
            assert!(breakdown.is_none());
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

/// The breakdown round-trips value-identically through serialize→deserialize.
#[test]
fn breakdown_round_trips() {
    let b = TrackCostBreakdown {
        surface: 1.0,
        elevated: 2.0,
        cut_cover: 3.0,
        bored: 4.0,
        strata: "rock".to_string(),
        below_water_table: true,
    };
    let s = serde_json::to_string(&b).unwrap();
    let b2: TrackCostBreakdown = serde_json::from_str(&s).unwrap();
    assert_eq!(b, b2);
}

/// The `strataProbe` client query serializes to the right envelope.
#[test]
fn strata_probe_query_to_envelope() {
    let msg = ToSim::StrataProbe {
        seq: 3,
        payload: StrataProbePayload {
            x: 100.0,
            y: -250.0,
        },
    };
    let env = msg.to_envelope();
    assert_eq!(env.t, "strataProbe");
    assert_eq!(env.seq, Some(3));
    let p = env.p.expect("payload");
    assert_eq!(p["x"], 100.0);
    assert_eq!(p["y"], -250.0);
}

/// A `strataProbe` reply decodes through the envelope into a full column.
#[test]
fn strata_probe_reply_decodes() {
    let env = Envelope {
        t: "strataProbe".to_string(),
        seq: Some(3),
        p: Some(json!({
            "bands": [
                {"kind": "fill", "top": 0.0, "bottom": 4.0},
                {"kind": "clay", "top": 4.0, "bottom": 12.0},
                {"kind": "rock", "top": 12.0, "bottom": 200.0},
                {"kind": "bedrock", "top": 200.0, "bottom": 1000.0}
            ],
            "waterTable": 8.5,
            "rockHardness": 0.88,
            "surfaceElevation": 14.0
        })),
    };
    match FromSimJson::from_envelope(env).expect("decode") {
        FromSimJson::StrataProbe { seq, result } => {
            assert_eq!(seq, Some(3));
            assert_eq!(result.bands.len(), 4);
            assert_eq!(result.bands[2].kind, "rock");
            assert_eq!(result.water_table, 8.5);
            assert_eq!(result.rock_hardness, 0.88);
            assert_eq!(result.surface_elevation, 14.0);
        }
        other => panic!("wrong variant: {other:?}"),
    }
}

/// The reply payload round-trips value-identically.
#[test]
fn strata_probe_result_round_trips() {
    let r = StrataProbeResultPayload {
        bands: vec![],
        water_table: 5.0,
        rock_hardness: 0.5,
        surface_elevation: 3.0,
    };
    let s = serde_json::to_string(&r).unwrap();
    let r2: StrataProbeResultPayload = serde_json::from_str(&s).unwrap();
    assert_eq!(r, r2);
}
