//! P5 OSM real-city acceptance: loading `nyc` populates non-empty masks /
//! elevation / anchors of the expected resolution, is run-twice identical
//! (static input, no RNG), and the decoded set-cell / label / anchor counts
//! match the TS `osmCity` output for the same bundle (captured via python from
//! `sim/src/data/cities/nyc.json`).

use mf_net::cities::resolve_city;
use mf_net::host;
use mf_sim::types::Difficulty;
use mf_sim::{new_game, NewGameOptions};

fn nyc_opts() -> NewGameOptions {
    NewGameOptions {
        preset_key: Some("nyc".into()),
        osm: resolve_city(Some("nyc")),
        ..Default::default()
    }
}

fn set_cells(m: &[u8]) -> usize {
    m.iter().filter(|&&b| b == 1).count()
}

#[test]
fn nyc_loads_real_masks_elevation_and_anchors() {
    let s = new_game(12345, Difficulty::Normal, nyc_opts());

    // resolution
    assert_eq!(s.osm_mask_res, Some(640), "mask res");
    assert_eq!(s.osm_elev_res, Some(256), "elev res");

    // masks present + expected size
    let water = s.osm_water_mask.as_ref().expect("water mask");
    let park = s.osm_park_mask.as_ref().expect("park mask");
    let building = s.osm_building_mask.as_ref().expect("building mask");
    assert_eq!(water.len(), 640 * 640);
    assert_eq!(park.len(), 640 * 640);
    assert_eq!(building.len(), 640 * 640);

    // set-cell counts vs TS decode of the same bundle (exact — pure decode)
    assert_eq!(set_cells(water), 73838, "water set cells");
    assert_eq!(set_cells(park), 28636, "park set cells");
    assert_eq!(set_cells(building), 93046, "building set cells");

    // elevation
    let elev = s.osm_elevation.as_ref().expect("elevation");
    assert_eq!(elev.len(), 256 * 256);
    assert!(elev.iter().any(|&h| h != 0), "elevation non-trivial");

    // roads: real OSM network (all bundle segments with >=2 points)
    assert_eq!(s.roads.len(), 13871, "osm road count");

    // labels + POI anchors
    assert_eq!(
        s.osm_labels.as_ref().map(|l| l.len()),
        Some(513),
        "label count"
    );
    assert_eq!(
        s.poi_anchors.as_ref().map(|a| a.len()),
        Some(40),
        "poi anchor count"
    );

    // coarse field grid picked up real water (Manhattan is coastal)
    let coarse_water = set_cells(&s.fields.water);
    assert!(coarse_water > 0, "coarse water present: {coarse_water}");
}

#[test]
fn nyc_is_run_twice_identical() {
    let a = new_game(777, Difficulty::Normal, nyc_opts());
    let b = new_game(777, Difficulty::Normal, nyc_opts());
    assert_eq!(a.state_hash(), b.state_hash(), "state hash stable");
    assert_eq!(a.osm_water_mask, b.osm_water_mask, "water mask stable");
    assert_eq!(a.osm_building_mask, b.osm_building_mask, "building stable");
    assert_eq!(a.osm_elevation, b.osm_elevation, "elevation stable");
    assert_eq!(a.roads.len(), b.roads.len(), "roads stable");
    assert_eq!(a.poi_anchors, b.poi_anchors, "anchors stable");
}

#[test]
fn ready_and_binary_frames_reflect_real_city() {
    let s = new_game(1, Difficulty::Normal, nyc_opts());

    let ready = host::build_ready(&s);
    let sc = &ready.static_city;
    assert_eq!(sc.mask_res, Some(640));
    assert!(sc.has_water_mask && sc.has_park_mask && sc.has_building_mask);
    assert_eq!(sc.labels.as_ref().map(|l| l.len()), Some(513));
    assert_eq!(sc.poi_anchors.as_ref().map(|a| a.len()), Some(40));
    assert_eq!(sc.roads.len(), 13871);

    // three static mask frames (water/park/building) in sidecar order
    let masks = host::build_masks(&s);
    assert_eq!(masks.len(), 3);
    assert_eq!(masks[0].which, mf_protocol::MaskWhich::Water);
    assert_eq!(masks[0].res, 640);
    assert_eq!(masks[0].mask.len(), 640 * 640);

    // elevation frame
    let elev = host::build_elevation(&s).expect("elevation frame");
    assert_eq!(elev.res, 256);
    assert_eq!(elev.heights.len(), 256 * 256);

    // static building vectors (msgType=5) from city data.
    let buildings = host::build_static_buildings(Some("nyc")).expect("static buildings");
    assert!(buildings.buildings.len() > 10_000);
    // run-twice byte-identical framing (determinism + anti-cheat assumptions).
    let a = buildings.encode();
    let b = host::build_static_buildings(Some("nyc"))
        .expect("static buildings run 2")
        .encode();
    assert_eq!(a, b, "static buildings frame must be stable");
}

#[test]
fn procedural_city_has_no_osm_channels() {
    let s = new_game(
        42,
        Difficulty::Normal,
        NewGameOptions {
            preset_key: Some("generic".into()),
            ..Default::default()
        },
    );
    assert!(s.osm_water_mask.is_none());
    assert!(s.osm_mask_res.is_none());
    assert!(s.poi_anchors.is_none());
    assert!(host::build_masks(&s).is_empty());
    assert!(host::build_elevation(&s).is_none());
    // procedural roads still generated
    assert!(!s.roads.is_empty());
}
