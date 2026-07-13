//! Live-sidecar integration test.
//!
//! The TypeScript sidecar (`sim/sidecar/`, spec §2) lives in-repo; the dev
//! fallback builds it from there. This test is `#[ignore]`d by default so `cargo test` (and CI's `ci.yml`)
//! stays green standalone; once the sidecar lands, run it explicitly:
//!
//! ```sh
//! cargo test -p mf-net --test live_sidecar -- --ignored
//! ```
//!
//! Set `MF_REQUIRE_SIDECAR=1` to make a missing/failed sidecar a hard
//! failure instead of a skip (useful once a CI job is added that's
//! supposed to have the sidecar built already).

use std::time::{Duration, Instant};

use mf_net::SimLink;
use mf_protocol::envelope::FromSimJson;
use mf_protocol::{
    ClientHelloPayload, Difficulty, FromSimMsg, InitPayload, ToSim, PROTOCOL_VERSION,
};

#[test]
#[ignore = "requires the in-repo sim/ sidecar and bun; see module docs"]
fn connects_and_receives_hello() {
    let require = std::env::var("MF_REQUIRE_SIDECAR").is_ok();

    let link = match SimLink::spawn_and_connect(None) {
        Ok(link) => link,
        Err(e) => {
            if require {
                panic!("MF_REQUIRE_SIDECAR set but sidecar spawn/connect failed: {e}");
            }
            eprintln!("skipping connects_and_receives_hello: sidecar not available yet ({e})");
            return;
        }
    };

    link.transport
        .send(ToSim::Hello(ClientHelloPayload {
            client_protocol_version: PROTOCOL_VERSION,
        }))
        .expect("send hello");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got_hello = false;
    while Instant::now() < deadline {
        match link.transport.try_recv() {
            Some(FromSimMsg::Json(FromSimJson::Hello(info))) => {
                assert_eq!(info.protocol_version, PROTOCOL_VERSION);
                assert!(
                    !info.city_list.is_empty(),
                    "expected at least one city (nyc)"
                );
                got_hello = true;
                break;
            }
            Some(_) => continue,
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    assert!(
        got_hello,
        "did not receive a hello reply from the sidecar within 10s"
    );
}

/// Exercises the full Boot -> Loading handshake against the real sidecar:
/// `init` -> `ready` (+ any `StaticMask` frames) -> first `Fields` -> first
/// `UiState`. This is the strongest available check that `mf-protocol`'s
/// types actually decode the sidecar's real JSON/binary wire output, not
/// just the fixtures in `mf-protocol/tests/roundtrip.rs`.
#[test]
#[ignore = "requires the in-repo sim/ sidecar and bun; see module docs"]
fn inits_nyc_and_receives_ready_fields_and_ui() {
    let require = std::env::var("MF_REQUIRE_SIDECAR").is_ok();

    let link = match SimLink::spawn_and_connect(None) {
        Ok(link) => link,
        Err(e) => {
            if require {
                panic!("MF_REQUIRE_SIDECAR set but sidecar spawn/connect failed: {e}");
            }
            eprintln!("skipping inits_nyc_and_receives_ready_fields_and_ui: sidecar not available yet ({e})");
            return;
        }
    };

    link.transport
        .send(ToSim::Init(InitPayload {
            seed: 12345,
            difficulty: Difficulty::Normal,
            size: None,
            preset_key: Some("nyc".to_string()),
            rules: None,
            scenario_id: None,
        }))
        .expect("send init");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut got_ready = false;
    let mut got_fields = false;
    let mut got_ui = false;
    let mut mask_res: Option<u32> = None;
    let mut masks_seen = 0u32;

    while Instant::now() < deadline && !(got_ready && got_fields && got_ui) {
        match link.transport.try_recv() {
            Some(FromSimMsg::Json(FromSimJson::Ready(ready))) => {
                let city = ready.static_city;
                assert!(
                    city.field_w > 0 && city.field_h > 0,
                    "expected non-empty field grid"
                );
                assert!(city.world_size > 0.0);
                mask_res = city.mask_res;
                got_ready = true;
            }
            Some(FromSimMsg::Mask(mask)) => {
                if let Some(res) = mask_res {
                    assert_eq!(
                        mask.res, res,
                        "mask res should match StaticCityJson.mask_res"
                    );
                }
                assert_eq!(mask.mask.len(), (mask.res * mask.res) as usize);
                masks_seen += 1;
            }
            Some(FromSimMsg::Fields(fields)) => {
                assert_eq!(fields.terrain.len(), fields.cell_count as usize);
                assert_eq!(fields.water.len(), fields.cell_count as usize);
                got_fields = true;
            }
            Some(FromSimMsg::Json(FromSimJson::Ui(ui))) => {
                // Successfully decoding into `UiState` at all is the point of
                // this branch.
                eprintln!("ui: tick={} day={} cash={:.2}", ui.tick, ui.day, ui.cash);
                got_ui = true;
            }
            Some(_) => continue,
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    assert!(got_ready, "never received `ready`");
    assert!(got_fields, "never received a `Fields` binary frame");
    assert!(got_ui, "never received a `ui` UiState");
    eprintln!("received {masks_seen} StaticMask frame(s)");
}

/// Boot check (#25): a data-driven scenario's rules must land on the first
/// `ui` snapshot (starting cash, calendar limit, unlocked modes).
#[test]
#[ignore = "requires the in-repo sim/ sidecar and bun; see module docs"]
fn scenario_rules_apply_on_init() {
    use mf_protocol::{ScenarioRules, TransitMode};

    let require = std::env::var("MF_REQUIRE_SIDECAR").is_ok();

    let link = match SimLink::spawn_and_connect(None) {
        Ok(link) => link,
        Err(e) => {
            if require {
                panic!("MF_REQUIRE_SIDECAR set but sidecar spawn/connect failed: {e}");
            }
            eprintln!("skipping scenario_rules_apply_on_init: sidecar not available yet ({e})");
            return;
        }
    };

    let rules = ScenarioRules {
        scenario_id: Some("cleveland-first-riders".to_string()),
        starting_modes: vec![TransitMode::Bus],
        lock_modes: Some(true),
        max_day: Some(45),
        approval_floor: None,
        starting_cash: Some(12_000_000.0),
        daily_subsidy: Some(35_000.0),
        era_label: Some("Starter".to_string()),
    };

    link.transport
        .send(ToSim::Init(InitPayload {
            seed: 4242,
            difficulty: Difficulty::Easy,
            size: None,
            preset_key: Some("cleveland".to_string()),
            rules: Some(rules),
            scenario_id: Some("cleveland-first-riders".to_string()),
        }))
        .expect("send init");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut got_ui = false;

    while Instant::now() < deadline && !got_ui {
        match link.transport.try_recv() {
            Some(FromSimMsg::Json(FromSimJson::Ui(ui))) => {
                assert!(
                    (ui.cash - 12_000_000.0).abs() < 1.0,
                    "expected scenario starting cash 12M, got {}",
                    ui.cash
                );
                assert_eq!(ui.max_day, Some(45), "expected scenario deadline 45 days");
                assert_eq!(
                    ui.unlocked_modes,
                    vec![TransitMode::Bus],
                    "expected bus-only start"
                );
                got_ui = true;
            }
            Some(_) => continue,
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }

    assert!(
        got_ui,
        "never received a `ui` UiState with scenario rules applied"
    );
}
