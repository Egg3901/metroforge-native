//! Real-city OSM bundle registry + JSON loader for the embedded sim.
//!
//! This is the native-host analogue of `sim/sidecar/cities.ts`: it resolves a
//! preset key (`"nyc"`, `"boston"`, ...) to a parsed [`OsmCityData`] the sim can
//! consume. `mf-sim` stays serde-free, so the JSON parse + base64 payloads live
//! here (mf-net already depends on serde_json).
//!
//! ## Data delivery
//! Embedding all ten multi-MB bundles (plus buildings) would bloat the binary
//! by ~90 MB, so we take the pragmatic split the task calls for:
//! * **NYC + Boston are embedded** via `include_bytes!` (~2 MB total) so the
//!   flagship path is single-binary and always available (incl. tests).
//! * **The other eight cities load from a data directory** at runtime,
//!   resolved from `$MF_CITY_DATA` if set, else the in-repo
//!   `sim/src/data/cities` path baked at compile time. Missing files fall back
//!   to procedural generation (return `None`).
//!
//! Loading is deterministic: pure parse of static bytes, stable ordering.

use mf_sim::city::osm::{OsmCityData, OsmRoad};
use mf_sim::types::{MapLabel, MapLabelKind, PoiAnchor, PoiKind};
use serde::Deserialize;

/// Embedded flagship bundles (single-binary friendly).
static NYC_JSON: &[u8] = include_bytes!("../../../sim/src/data/cities/nyc.json");
static BOSTON_JSON: &[u8] = include_bytes!("../../../sim/src/data/cities/boston.json");

/// Compile-time fallback data dir for the non-embedded cities.
const DEFAULT_DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../sim/src/data/cities");

/// The ten OSM-backed preset keys, in registry order (mirrors
/// `OSM_CITY_KEYS`). Used to answer "is this a real city?" cheaply.
pub const OSM_CITY_KEYS: &[&str] = &[
    "nyc",
    "boston",
    "chicago",
    "cleveland",
    "la",
    "atlanta",
    "philly",
    "sf",
    "dc",
    "seattle",
];

/// True if `key` names a real OSM-backed city.
pub fn is_osm_city(key: &str) -> bool {
    OSM_CITY_KEYS.contains(&key)
}

// ── JSON DTOs (camelCase, mirroring the bundle) ──────────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BundleDto {
    #[serde(default)]
    key: String,
    #[serde(default)]
    label: String,
    world_size: f64,
    mask_res: u32,
    #[serde(default)]
    mask_packed: bool,
    water_mask: String,
    #[serde(default)]
    park_mask: Option<String>,
    #[serde(default)]
    building_mask: Option<String>,
    #[serde(default)]
    elev_res: Option<u32>,
    #[serde(default)]
    elevation: Option<String>,
    #[serde(default)]
    roads: Vec<RoadDto>,
    #[serde(default)]
    labels: Vec<LabelDto>,
    #[serde(default)]
    poi_anchors: Vec<AnchorDto>,
}

#[derive(Deserialize)]
struct RoadDto {
    cls: String,
    pts: Vec<f64>,
    #[serde(default)]
    g: i32,
    #[serde(default)]
    br: bool,
    #[serde(default)]
    tn: bool,
}

#[derive(Deserialize)]
struct LabelDto {
    kind: String,
    name: String,
    x: f64,
    y: f64,
    #[serde(default)]
    angle: Option<f64>,
    imp: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnchorDto {
    id: String,
    kind: String,
    name: String,
    centroid: [f64; 2],
    #[serde(default)]
    area: Option<f64>,
}

fn label_kind(s: &str) -> MapLabelKind {
    match s {
        "water" => MapLabelKind::Water,
        "park" => MapLabelKind::Park,
        _ => MapLabelKind::Road,
    }
}

/// Map an OSM anchor kind onto [`PoiKind`]. Unknown kinds are dropped (return
/// `None`) so a future bundle kind never mis-maps to a wrong glyph.
fn poi_kind(s: &str) -> Option<PoiKind> {
    Some(match s {
        "stadium" => PoiKind::Stadium,
        "airport" => PoiKind::Airport,
        "university" => PoiKind::University,
        "hospital" => PoiKind::Hospital,
        "museum" => PoiKind::Museum,
        _ => return None,
    })
}

fn convert(dto: BundleDto) -> OsmCityData {
    let roads = dto
        .roads
        .into_iter()
        .map(|r| OsmRoad {
            cls: r.cls,
            pts: r.pts,
            g: r.g,
            br: r.br,
            tn: r.tn,
        })
        .collect();
    let labels = dto
        .labels
        .into_iter()
        .map(|l| MapLabel {
            kind: label_kind(&l.kind),
            name: l.name,
            x: l.x,
            y: l.y,
            angle: l.angle,
            imp: l.imp,
        })
        .collect();
    let poi_anchors = dto
        .poi_anchors
        .into_iter()
        .filter_map(|a| {
            poi_kind(&a.kind).map(|kind| PoiAnchor {
                id: a.id,
                kind,
                name: a.name,
                centroid: a.centroid,
                area: a.area,
            })
        })
        .collect();
    OsmCityData {
        key: dto.key,
        label: dto.label,
        world_size: dto.world_size,
        mask_res: dto.mask_res,
        mask_packed: dto.mask_packed,
        water_mask: dto.water_mask,
        park_mask: dto.park_mask,
        building_mask: dto.building_mask,
        elev_res: dto.elev_res,
        elevation: dto.elevation,
        roads,
        labels,
        poi_anchors,
    }
}

fn parse(bytes: &[u8]) -> Option<OsmCityData> {
    match serde_json::from_slice::<BundleDto>(bytes) {
        Ok(dto) => Some(convert(dto)),
        Err(e) => {
            tracing::warn!("failed to parse OSM city bundle: {e}");
            None
        }
    }
}

/// Resolve a preset key to a parsed OSM bundle. Embedded flagships (NYC,
/// Boston) always resolve; the other cities load from the data dir. Returns
/// `None` for procedural keys or a missing/corrupt data file (caller then
/// generates procedurally).
pub fn resolve_city(key: Option<&str>) -> Option<OsmCityData> {
    let key = key?;
    match key {
        "nyc" => parse(NYC_JSON),
        "boston" => parse(BOSTON_JSON),
        k if is_osm_city(k) => {
            let dir =
                std::env::var("MF_CITY_DATA").unwrap_or_else(|_| DEFAULT_DATA_DIR.to_string());
            let path = std::path::Path::new(&dir).join(format!("{k}.json"));
            match std::fs::read(&path) {
                Ok(bytes) => parse(&bytes),
                Err(_) => {
                    tracing::info!(
                        "OSM bundle for '{k}' not found at {}; using procedural fallback",
                        path.display()
                    );
                    None
                }
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nyc_bundle_parses_and_is_populated() {
        let osm = resolve_city(Some("nyc")).expect("nyc embedded");
        assert_eq!(osm.key, "nyc");
        assert_eq!(osm.world_size, 12000.0);
        assert_eq!(osm.mask_res, 640);
        assert!(osm.mask_packed);
        assert!(osm.roads.len() > 5000, "roads: {}", osm.roads.len());
        assert!(osm.labels.len() > 100, "labels: {}", osm.labels.len());
        assert!(!osm.poi_anchors.is_empty());
        assert!(osm.elevation.is_some() && osm.elev_res == Some(256));
    }

    #[test]
    fn boston_bundle_parses() {
        let osm = resolve_city(Some("boston")).expect("boston embedded");
        assert_eq!(osm.key, "boston");
        assert!(!osm.roads.is_empty());
    }

    #[test]
    fn procedural_key_is_none() {
        assert!(resolve_city(Some("generic")).is_none());
        assert!(resolve_city(None).is_none());
    }
}
